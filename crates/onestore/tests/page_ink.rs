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
        raster_operation: None,
        pressure: Vec::new(),
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
        shape: None,
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

/// OneNote 2010's Draw tab used with the mouse (`corpus/ink-tools/native-ui`): every favourite
/// pen and highlighter, shapes, and ink erased, moved and deleted.
const TOOLS: &[u8] = include_bytes!("../../../corpus/ink-tools/native-ui/notebook/ink.one");

fn titled(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| (space, Page::from_space(&document, space).unwrap()))
        .find(|(_, page)| page.title == title)
        .unwrap()
}

fn himetric(value: f32) -> f32 {
    value * 72.0 / 2540.0
}

use onestore::page::ink::{InkShape, ShapeKind};

#[test]
fn native_pens_highlighters_and_shapes_read_as_drawn() {
    let (_, gallery) = titled(TOOLS, "Gallery");
    let pens: Vec<_> = drawings(&gallery)
        .iter()
        .map(|ink| {
            let [stroke] = ink.strokes.as_slice() else {
                panic!("OneNote keeps each stroke a drawing of its own")
            };
            (
                (stroke.width / himetric(1.0)).round(),
                (stroke.height / himetric(1.0)).round(),
                stroke.color,
                stroke.raster_operation,
            )
        })
        .collect();
    let (red, blue, green, grey) = (0x241ced, 0xbb6531, 0x367d17, 0x808080);
    let pen = |width, color| (width, width, color, None);
    let marker = |color| (70.0, 400.0, Some(color), Some(9));
    assert_eq!(
        pens,
        [
            pen(35.0, None),
            pen(35.0, Some(red)),
            pen(35.0, Some(blue)),
            pen(35.0, Some(green)),
            pen(35.0, Some(grey)),
            marker(0x00ffff),
            marker(0xffff00),
            pen(50.0, None),
            pen(50.0, Some(red)),
            pen(50.0, Some(blue)),
            pen(50.0, Some(green)),
            pen(50.0, Some(grey)),
            marker(0x00ff00),
            marker(0xff00ff),
        ]
    );
    let highlighter = &drawings(&gallery)[5].strokes[0];
    assert_eq!(
        (highlighter.transparency, highlighter.pen_tip),
        (Some(127), Some(1))
    );

    let (_, pens) = titled(TOOLS, "Pens");
    let shapes: Vec<&Ink> = drawings(&pens)
        .into_iter()
        .filter(|ink| ink.shape.is_some())
        .collect();
    let [rectangle, line, arrow, ellipse] = shapes.as_slice() else {
        panic!("{}", shapes.len())
    };
    let pen = InkStroke {
        points: Vec::new(),
        ..rectangle.strokes[0].clone()
    };
    assert!((pen.width - himetric(50.0)).abs() < 1e-5);
    let close_points = |a: &[[f32; 2]], b: &[[f32; 2]]| {
        a.len() == b.len()
            && a.iter()
                .zip(b)
                .all(|(a, b)| (a[0] - b[0]).abs() < 0.05 && (a[1] - b[1]).abs() < 0.05)
    };
    for (native, kind, from, to) in [
        (
            rectangle,
            ShapeKind::Rectangle,
            [252.0, 86.4],
            [324.0, 122.4],
        ),
        (line, ShapeKind::Line, [342.0, 86.4], [414.0, 122.4]),
        (arrow, ShapeKind::Arrow, [252.0, 158.4], [324.0, 194.4]),
        (ellipse, ShapeKind::Ellipse, [342.0, 158.4], [414.0, 194.4]),
    ] {
        let drawn = Ink::drawn(kind, from, to, &pen).unwrap();
        assert_eq!(drawn.strokes.len(), native.strokes.len(), "{kind:?}");
        for (drawn, native) in drawn.strokes.iter().zip(&native.strokes) {
            assert!(
                close_points(&drawn.points, &native.points),
                "{kind:?}: {:?} against {:?}",
                drawn.points,
                native.points
            );
        }
        match (drawn.shape.unwrap(), native.shape.clone().unwrap()) {
            (InkShape::Line(a), InkShape::Line(b)) => assert!(close_points(&a, &b)),
            (
                InkShape::Closed { transform, anchors },
                InkShape::Closed {
                    transform: native_transform,
                    anchors: native_anchors,
                },
            ) => {
                assert_eq!(anchors, native_anchors);
                assert!(
                    transform
                        .iter()
                        .zip(native_transform)
                        .all(|(a, b)| (a - b).abs() < 0.05)
                );
            }
            shapes => panic!("{shapes:?}"),
        }
    }

    // Erased and deleted drawings leave the page; a moved one keeps its strokes and takes
    // an offset.
    let (_, edits) = titled(TOOLS, "Edits");
    let moved: Vec<_> = drawings(&edits)
        .into_iter()
        .filter(|ink| ink.shape.is_none())
        .map(|ink| (ink.layout.x, ink.layout.y))
        .collect();
    assert_eq!(moved.len(), 2);
    assert!(
        moved
            .iter()
            .any(|&(x, y)| x == Some(45.0) && y.is_some_and(|y| (y - 15.0).abs() < 1e-4))
    );
}

fn tool_page(source: &[u8]) -> (ExGuid, Page, Vec<Ink>) {
    let (space, page) = first_page(source);
    let accent = InkStroke {
        id: new_id().unwrap(),
        points: Vec::new(),
        width: snap(himetric(35.0)),
        height: snap(himetric(35.0)),
        color: Some(0x7a9a1f),
        transparency: None,
        pen_tip: None,
        raster_operation: None,
        pressure: Vec::new(),
    };
    let marker = InkStroke {
        width: himetric(70.0),
        height: himetric(400.0),
        color: Some(0x00ffff),
        transparency: Some(127),
        pen_tip: Some(1),
        raster_operation: Some(9),
        ..accent.clone()
    };
    let shape_pen = InkStroke {
        width: himetric(50.0),
        height: himetric(50.0),
        ..accent.clone()
    };
    let free = |pen: &InkStroke, points: Vec<[f32; 2]>| Ink {
        id: new_id().unwrap(),
        layout: Default::default(),
        strokes: vec![InkStroke {
            id: new_id().unwrap(),
            points: points.into_iter().map(|p| p.map(snap)).collect(),
            ..pen.clone()
        }],
        groups: Vec::new(),
        shape: None,
    };
    let wave: Vec<[f32; 2]> = (0..=40)
        .map(|step| {
            let x = 60.0 + step as f32 * 4.0;
            [x, 150.0 + 12.0 * (x / 18.0).sin()]
        })
        .collect();
    let inks = vec![
        free(&marker, vec![[60.0, 190.0], [220.0, 190.0]]),
        free(&accent, wave),
        Ink::drawn(
            ShapeKind::Rectangle,
            [252.0, 140.4],
            [324.0, 194.4],
            &shape_pen,
        )
        .unwrap(),
        Ink::drawn(
            ShapeKind::Ellipse,
            [342.0, 140.4],
            [432.0, 194.4],
            &shape_pen,
        )
        .unwrap(),
        Ink::drawn(ShapeKind::Arrow, [252.0, 230.4], [360.0, 266.4], &shape_pen).unwrap(),
        Ink::drawn(ShapeKind::Line, [378.0, 230.4], [450.0, 230.4], &shape_pen).unwrap(),
    ];
    (space, page, inks)
}

fn added(source: &[u8], space: ExGuid, inks: &[Ink]) -> Vec<u8> {
    let mut bytes = source.to_vec();
    for ink in inks {
        bytes = ops::page_edited(
            &bytes,
            space,
            vec![onestore::op::PageOp::Add {
                object: PageObject::Ink(ink.clone()),
                before: None,
            }],
        )
        .unwrap();
    }
    bytes
}

/// `ONESTORE_INK_TOOLS_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn pens_highlighters_and_shapes_are_written_as_onenote_draws_them() {
    use onestore::document::{FieldValue, Kind};
    let source = onestore::create_section("ink.one", "Drawn in Snowbound", "Author").unwrap();
    let (space, before, inks) = tool_page(&source);
    // Each stroke is one edit, as the canvas stores it.
    let written = added(&source, space, &inks);
    let stored = page_in(&written, space);
    let mut expected = before.clone();
    expected
        .objects
        .extend(inks.iter().cloned().map(PageObject::Ink));
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    export("ONESTORE_INK_TOOLS_EXPORT", "ink.one", &written);

    let store = Store::parse(&written).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let revision = document.active(space).unwrap();
    let node = |id: ExGuid| &revision.nodes[&id];
    let extra = |id: ExGuid, property: u32| {
        node(id)
            .extra
            .iter()
            .flatten()
            .find(|field| field.id == property)
            .map(|field| match field.value {
                FieldValue::Bytes(bytes) => bytes.to_vec(),
                _ => Vec::new(),
            })
    };
    let style = |stroke: ExGuid| match node(stroke).kind {
        Kind::InkStroke {
            style: Some(style),
            bias,
            index,
            ..
        } => (node(style).kind.clone(), bias, index),
        _ => panic!(),
    };
    let [marker, wave, rectangle, _, arrow, _] = inks.as_slice() else {
        unreachable!()
    };
    match style(marker.strokes[0].id) {
        (
            Kind::InkStyle {
                raster_operation: Some(9),
                pen_tip: Some(1),
                transparency: Some(127),
                ignore_pressure: Some(true),
                ..
            },
            Some(2),
            Some(1),
        ) => {}
        other => panic!("{other:?}"),
    }
    match style(wave.strokes[0].id) {
        (
            Kind::InkStyle {
                raster_operation: None,
                ignore_pressure: Some(true),
                color: Some(0x7a9a1f),
                ..
            },
            Some(0),
            Some(2),
        ) => {}
        other => panic!("{other:?}"),
    }
    for stroke in &arrow.strokes {
        match style(stroke.id) {
            (
                Kind::InkStyle {
                    ignore_pressure: None,
                    ..
                },
                None,
                None,
            ) => {}
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(
        extra(wave.id, 0x14001d4e),
        Some(1u32.to_le_bytes().to_vec())
    );
    assert_eq!(
        extra(rectangle.id, 0x14001d4e),
        Some(2u32.to_le_bytes().to_vec())
    );
    match &node(rectangle.id).kind {
        Kind::Ink {
            shape_kind: Some(12),
            anchors: Some(anchors),
            ..
        } => assert_eq!(anchors.len(), 24 + 8 * 8),
        other => panic!("{other:?}"),
    }
}

/// `ONESTORE_INK_TOOLS_EDIT_EXPORT` names a new directory receiving the candidate for a cold
/// reopen: OneNote's own drawings after Snowbound erases one, moves another and draws more.
#[test]
fn onenote_drawings_take_erasing_moving_and_more_strokes() {
    use onestore::{OutlineEdit, op::PageOp};
    let (space, page) = titled(TOOLS, "Pens");
    let native = drawings(&page);
    let zigzag = native
        .iter()
        .find(|ink| ink.strokes[0].color == Some(0x241ced))
        .unwrap()
        .id;
    let line = native[1].id;
    let edited = ops::page_edited(
        TOOLS,
        space,
        vec![
            PageOp::Delete { object: zigzag },
            PageOp::Outline {
                object: line,
                edit: OutlineEdit::Position { x: 36.0, y: 18.0 },
            },
        ],
    )
    .unwrap();
    let (_, _, inks) = tool_page(&onestore::create_section("x.one", "x", "x").unwrap());
    let moved: Vec<Ink> = inks
        .into_iter()
        .map(|mut ink| {
            for stroke in &mut ink.strokes {
                for point in &mut stroke.points {
                    point[1] = snap(point[1] + 200.0);
                }
            }
            if let Some(shape) = &mut ink.shape {
                *shape = match shape {
                    InkShape::Line([a, b]) => {
                        InkShape::Line([[a[0], a[1] + 200.0], [b[0], b[1] + 200.0]])
                    }
                    InkShape::Closed { transform, anchors } => InkShape::Closed {
                        transform: {
                            let mut t = *transform;
                            t[5] += 200.0;
                            t
                        },
                        anchors: anchors.clone(),
                    },
                }
                .snapped();
            }
            ink
        })
        .collect();
    let written = added(&edited, space, &moved);
    let stored = page_in(&written, space);
    let found = drawings(&stored);
    assert!(!found.iter().any(|ink| ink.id == zigzag));
    let line = found.iter().find(|ink| ink.id == line).unwrap();
    assert_eq!((line.layout.x, line.layout.y), (Some(36.0), Some(18.0)));
    assert_eq!(line.strokes, native[1].strokes);
    for ink in &moved {
        assert_eq!(found.iter().find(|found| found.id == ink.id), Some(&ink));
    }
    export("ONESTORE_INK_TOOLS_EDIT_EXPORT", "ink.one", &written);
}

/// OneNote 2010's pressure ink, taken in from ISF (`tools/native/ink-pressure.ps1`).
const PRESSURE: &[u8] = include_bytes!("../../../corpus/ink-pressure/native/notebook/Pressure.one");

#[test]
fn onenote_pressure_ink_reads_each_point_s_level_over_the_pen_s_range() {
    let (_, page) = titled(PRESSURE, "Levels");
    let [fine, coarse] = drawings(&page)[..] else {
        panic!("{:?}", drawings(&page).len());
    };
    for (ink, range) in [(fine, 1023.0), (coarse, 255.0)] {
        assert_eq!(ink.strokes.len(), 9);
        for (step, stroke) in ink.strokes.iter().enumerate() {
            let level = (step as f32 / 8.0 * range).round() / range;
            assert_eq!(stroke.pressure.len(), stroke.points.len());
            assert!(stroke.pressure.iter().all(|p| (p - level).abs() < 1e-6));
            assert!((stroke.thickness(0) - (0.25 + 1.5 * level)).abs() < 1e-6);
        }
    }
    let (_, page) = titled(PRESSURE, "Strokes");
    let [ramp, _, flat, tilt, block] = drawings(&page)[..] else {
        panic!("{:?}", drawings(&page).len());
    };
    let ramp = &ramp.strokes[0].pressure;
    assert_eq!((ramp.len(), ramp[0], ramp[40]), (41, 0.0, 1.0));
    // A pen that ignores the pressure it recorded draws at its width throughout.
    assert!(flat.strokes[0].pressure.is_empty());
    assert_eq!(flat.strokes[0].thickness(20), 1.0);
    // Tilt beside the pressure leaves the pressure as it is.
    let tilted = &tilt.strokes[0].pressure;
    assert!((tilted[0] - 205.0 / 1023.0).abs() < 1e-6, "{tilted:?}");
    assert_eq!(block.strokes[0].pen_tip, Some(1));
    assert_eq!(block.strokes[0].pressure.len(), 11);
}

/// `ONESTORE_INK_PRESSURE_EXPORT` names a new directory receiving the candidate for a cold
/// reopen: OneNote's pressure ink after Snowbound erases a stroke of one drawing, moves one
/// and deletes another, beside Snowbound's own pressure strokes.
#[test]
fn pressure_ink_is_written_as_onenote_keeps_it_and_survives_edits() {
    use onestore::document::Kind;
    use onestore::{OutlineEdit, op::PageOp};
    let (levels_space, levels) = titled(PRESSURE, "Levels");
    let fine = drawings(&levels)[0].clone();
    let erased = ops::page_edited(
        PRESSURE,
        levels_space,
        vec![PageOp::Strokes {
            ink: fine.id,
            add: Vec::new(),
            remove: vec![fine.strokes[0].id],
        }],
    )
    .unwrap();
    let pen = |points: Vec<[f32; 2]>, pressure: Vec<f32>| Ink {
        id: new_id().unwrap(),
        layout: Default::default(),
        strokes: vec![InkStroke {
            id: new_id().unwrap(),
            points: points.into_iter().map(|p| p.map(snap)).collect(),
            width: snap(himetric(500.0)),
            height: snap(himetric(500.0)),
            color: Some(0x7a9a1f),
            transparency: None,
            pen_tip: None,
            raster_operation: None,
            pressure: pressure
                .into_iter()
                .map(onestore::page::ink::level)
                .collect(),
        }],
        groups: Vec::new(),
        shape: None,
    };
    // Constant levels of none, half and full pressure, and a ramp from none to full.
    let mut inks: Vec<Ink> = [0.0, 0.5, 1.0]
        .into_iter()
        .enumerate()
        .map(|(row, pressure)| {
            let y = 520.0 + 36.0 * row as f32;
            pen(vec![[36.0, y], [108.0, y], [180.0, y]], vec![pressure; 3])
        })
        .collect();
    inks.push(pen(
        (0..=40)
            .map(|step| [36.0 + 6.0 * step as f32, 640.0])
            .collect(),
        (0..=40).map(|step| step as f32 / 40.0).collect(),
    ));
    let written = added(&erased, levels_space, &inks);
    let (strokes_space, strokes) = titled(&written, "Strokes");
    let [ramp, wave, ..] = drawings(&strokes)[..] else {
        unreachable!()
    };
    let written = ops::page_edited(
        &written,
        strokes_space,
        vec![
            PageOp::Delete { object: wave.id },
            PageOp::Outline {
                object: ramp.id,
                edit: OutlineEdit::Position { x: 72.0, y: 108.0 },
            },
        ],
    )
    .unwrap();

    let stored = page_in(&written, levels_space);
    let found = drawings(&stored);
    let kept = found.iter().find(|ink| ink.id == fine.id).unwrap();
    assert_eq!(kept.strokes, fine.strokes[1..]);
    for ink in &inks {
        assert_eq!(found.iter().find(|found| found.id == ink.id), Some(&ink));
    }
    let stored = page_in(&written, strokes_space);
    let moved = drawings(&stored);
    assert_eq!(moved.len(), 4);
    assert_eq!(
        (moved[0].layout.x, moved[0].layout.y),
        (Some(72.0), Some(108.0))
    );
    assert_eq!(moved[0].strokes, ramp.strokes);

    // New pressure strokes store NormalPressure after X and Y and leave IgnorePressure out,
    // as OneNote does with pressure sensitivity on; OneNote's tilt stays as it stored it.
    let store = Store::parse(&written).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let style = |space: ExGuid, stroke: ExGuid| {
        let revision = document.active(space).unwrap();
        let Kind::InkStroke {
            style: Some(style), ..
        } = revision.nodes[&stroke].kind
        else {
            panic!()
        };
        revision.nodes[&style].kind.clone()
    };
    match style(levels_space, inks[0].strokes[0].id) {
        Kind::InkStyle {
            dimensions,
            ignore_pressure: None,
            ..
        } => {
            assert_eq!(dimensions.len(), 96);
            assert_eq!(
                &dimensions[64..],
                &[
                    0x2d, 0x50, 0x07, 0x73, 0xf4, 0xf9, 0x18, 0x4e, 0xb3, 0xf2, 0x2c, 0xe1, 0xb1,
                    0xa3, 0x61, 0x0c, 0, 0, 0, 0, 0xff, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0x80, 0x3f
                ]
            );
        }
        other => panic!("{other:?}"),
    }
    match style(strokes_space, moved[2].strokes[0].id) {
        Kind::InkStyle { dimensions, .. } => assert_eq!(dimensions.len(), 5 * 32),
        other => panic!("{other:?}"),
    }
    export("ONESTORE_INK_PRESSURE_EXPORT", "Pressure.one", &written);
}
