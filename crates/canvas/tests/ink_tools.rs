//! The Draw tab's tools through the page view, as a host drives them: every stroke and shape
//! is one edit adding one drawing, the eraser and the lasso's Delete remove drawings, a lasso
//! drag moves them, and each is one undo step. The ops seal into a section reading back as the
//! editor's page. `CANVAS_INK_EXPORT` names a directory receiving that section as a notebook,
//! for a cold reopen in OneNote 2010 (`corpus/ink-tools`).
#![cfg(feature = "interaction")]

use canvas::{
    editor::{FAVORITES, Pen},
    gpu::page::PageScene,
    interaction::{PageView, TextColors, ink::Tool},
    layout::TextEngine,
};
use draw::edit::{Key, NamedKey};
use onestore::{
    Arena, ExGuid, Section, Store,
    op::{Edit, Op, PageOp},
    page::{Page, PageObject, ink::ShapeKind},
};
use std::time::{Duration, Instant};

const COLORS: TextColors = TextColors {
    caret: [0.0, 0.0, 0.0, 1.0],
    selection: [0.7, 0.8, 1.0, 1.0],
    paper: canvas::gpu::Paper::WHITE,
};

struct Lab<'a> {
    section: Section<'a>,
    space: ExGuid,
    view: PageView,
    at: u64,
}

impl Lab<'_> {
    /// Stores what the last gesture recorded, returning its ops.
    fn store(&mut self) -> Vec<PageOp> {
        let ops = self.view.editor.take_ops().unwrap();
        self.at += 10_000_000;
        let edit = Edit {
            at: self.at,
            ops: ops
                .iter()
                .cloned()
                .map(|op| Op::Page {
                    space: self.space,
                    op,
                })
                .collect(),
        };
        self.section.apply("Author", &edit).unwrap();
        assert_eq!(
            self.section.page(self.space).unwrap(),
            self.view.editor.page().unwrap()
        );
        ops
    }

    /// A press at page point `from` dragged through `through`, in a view at 1 device pixel
    /// per point.
    fn drag(&mut self, path: &[[f32; 2]]) {
        let view = &mut self.view;
        let _ = view.pointer_moved(path[0]).unwrap();
        let _ = view.pointer_pressed(Instant::now()).unwrap();
        for point in &path[1..] {
            let _ = view.pointer_moved(*point).unwrap();
        }
        let _ = view.pointer_released().unwrap();
    }

    fn drawings(&self) -> Vec<onestore::page::Ink> {
        self.view
            .editor
            .page()
            .unwrap()
            .objects
            .into_iter()
            .filter_map(|object| match object {
                PageObject::Ink(ink) => Some(ink),
                _ => None,
            })
            .collect()
    }
}

fn open(arena: &Arena, source: Vec<u8>) -> Lab<'_> {
    let mut section = Section::open(arena, source).unwrap();
    let (space, ..) = section.pages().unwrap()[0].clone();
    let page: Page = section.page(space).unwrap();
    let mut engine = TextEngine::default();
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    Lab {
        section,
        space,
        view,
        at: 133_000_000_000_000_000,
    }
}

fn line(from: [f32; 2], to: [f32; 2], steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let t = step as f32 / steps as f32;
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ]
        })
        .collect()
}

#[test]
fn drawing_tools_store_each_stroke_and_undo_it_in_one_step() {
    let source = onestore::create_section("ink.one", "Drawn in Snowbound", "Author").unwrap();
    let arena = Arena::default();
    let mut lab = open(&arena, source.clone());
    let accent = Pen::new(35.0, Some(0x7a9a1f));

    let _ = lab.view.set_tool(Tool::Pen(accent));
    let wave: Vec<[f32; 2]> = (0..=40)
        .map(|step| {
            let x = 60.0 + step as f32 * 4.0;
            [x, 150.0 + 12.0 * (x / 18.0).sin()]
        })
        .collect();
    lab.drag(&wave);
    let ops = lab.store();
    let [
        PageOp::Add {
            object: PageObject::Ink(ink),
            ..
        },
    ] = ops.as_slice()
    else {
        panic!("{ops:?}")
    };
    assert_eq!((ink.strokes.len(), ink.strokes[0].points.len()), (1, 41));
    assert_eq!(ink.strokes[0].color, Some(0x7a9a1f));

    let _ = lab.view.set_tool(Tool::Pen(FAVORITES[5]));
    lab.drag(&line([60.0, 190.0], [220.0, 190.0], 20));
    lab.store();
    for (kind, from, to) in [
        (ShapeKind::Rectangle, [250.0, 140.0], [326.0, 196.0]),
        (ShapeKind::Ellipse, [340.0, 140.0], [430.0, 196.0]),
        (ShapeKind::Arrow, [250.0, 230.0], [358.0, 266.0]),
        (ShapeKind::Line, [378.0, 230.0], [450.0, 232.0]),
    ] {
        let _ = lab
            .view
            .set_tool(Tool::Shape(kind, Pen::new(50.0, Some(0x7a9a1f))));
        lab.drag(&line(from, to, 8));
        lab.store();
    }
    let drawn = lab.drawings();
    assert_eq!(drawn.len(), 6);
    // Shapes take their corners from the grid, anchored at the margin origin.
    let rectangle = &drawn[2];
    assert_eq!(
        (
            rectangle.strokes[0].points[0],
            rectangle.strokes[0].points[2]
        ),
        (
            [252.0, 140.4].map(onestore::page::ink::snap),
            [324.0, 194.4].map(onestore::page::ink::snap)
        )
    );
    assert_eq!(
        drawn[4].strokes.len(),
        2,
        "an arrow's head is a stroke of its own"
    );
    let highlighter = drawn[1].strokes[0].clone();
    assert_eq!(
        (
            highlighter.raster_operation,
            highlighter.pen_tip,
            highlighter.transparency
        ),
        (Some(9), Some(1), Some(127))
    );
    // A highlighter multiplies what lies beneath.
    let primitives = lab.view.primitives(COLORS).unwrap();
    assert!(
        primitives
            .iter()
            .any(|primitive| matches!(primitive, draw::Primitive::Highlight { .. }))
    );
    if let Some(directory) = std::env::var_os("CANVAS_INK_EXPORT") {
        let mut image = source.clone();
        let mut sealed = Section::open(&arena, source.clone()).unwrap();
        std::mem::swap(&mut sealed, &mut lab.section);
        sealed.seal().unwrap().unwrap().apply(&mut image).unwrap();
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("ink.one"), &image).unwrap();
        let file_id = Store::parse(&image).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("ink.one", file_id)])
                .unwrap(),
        )
        .unwrap();
        return;
    }

    // Undo takes the last drawing away, and only that.
    let arrow = drawn[4].id;
    let line_id = drawn[5].id;
    let _ = lab.view.undo(false).unwrap();
    assert_eq!(lab.store(), [PageOp::Delete { object: line_id }]);
    let _ = lab.view.undo(true).unwrap();
    lab.store();
    assert_eq!(lab.drawings().len(), 6);

    // The eraser takes the wave it crosses, and the arrow's two strokes with it at a touch.
    let _ = lab.view.set_tool(Tool::Eraser);
    lab.drag(&[
        [100.0, 120.0],
        [100.0, 160.0],
        [240.0, 160.0],
        [240.0, 215.0],
        [320.0, 250.0],
    ]);
    let ops = lab.store();
    assert_eq!(
        ops,
        [
            PageOp::Delete {
                object: drawn[0].id
            },
            PageOp::Delete { object: arrow },
        ]
    );
    let _ = lab.view.undo(false).unwrap();
    lab.store();
    assert_eq!(lab.drawings().len(), 6);

    // The lasso picks the highlighter, a drag moves it, Delete removes it.
    let _ = lab.view.set_tool(Tool::Lasso);
    lab.drag(&[
        [50.0, 175.0],
        [230.0, 175.0],
        [230.0, 205.0],
        [50.0, 205.0],
        [50.0, 176.0],
    ]);
    assert_eq!(lab.view.ink_selection(), [drawn[1].id]);
    lab.drag(&line([140.0, 190.0], [170.0, 220.0], 4));
    let ops = lab.store();
    assert!(matches!(
        ops.as_slice(),
        [PageOp::Outline { object, .. }] if *object == drawn[1].id
    ));
    let moved = &lab.drawings()[1];
    assert_eq!((moved.layout.x, moved.layout.y), (Some(30.0), Some(30.0)));
    let _ = lab.view.key(&Key::Named(NamedKey::Delete), None).unwrap();
    assert_eq!(
        lab.store(),
        [PageOp::Delete {
            object: drawn[1].id
        }]
    );
    let _ = lab.view.undo(false).unwrap();
    lab.store();
    let _ = lab.view.undo(false).unwrap();
    lab.store();
    assert_eq!(lab.drawings()[1].layout.x, Some(0.0));

    // Escape leaves the drawing tools for Select & Type, where a click picks a drawing.
    let _ = lab.view.key(&Key::Named(NamedKey::Escape), None).unwrap();
    assert_eq!(lab.view.tool(), Tool::Select);
    lab.drag(&[[400.0, 231.0]]);
    assert!(lab.view.editor.take_ops().unwrap().is_empty());
    assert_eq!(lab.view.ink_selection(), [drawn[5].id]);
}
