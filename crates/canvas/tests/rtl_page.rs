//! A right-to-left page through the page view, as a host drives it. OneNote 2010 keeps such a
//! page's positions in the same left-to-right frame, its margin origin some 10,800 pt right of
//! the page origin, and opens it scrolled to the right end of its content; the view does the
//! same, and a click, a stroke and a drag store positions OneNote reads back where they were
//! put. `SNOWBOUND_RTL_EXPORT` names a new directory receiving the edited section as a notebook
//! for a cold reopen in OneNote 2010 (`corpus/rtl-page`).
#![cfg(feature = "interaction")]

use canvas::{
    editor::Pen,
    gpu::page::PageScene,
    interaction::{PageView, ink::Tool},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, Section, Store,
    op::{Edit, Op},
    page::{Page, PageObject},
};
use std::time::{Duration, Instant};

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/m6/native-page-direction-03/notebook/synthetic.one");

fn drag(view: &mut PageView, path: &[[f32; 2]]) {
    let _ = view.pointer_moved(path[0]).unwrap();
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    for point in &path[1..] {
        let _ = view.pointer_moved(*point).unwrap();
    }
    let _ = view.pointer_released().unwrap();
}

fn outline<'a>(page: &'a Page, text: &str) -> &'a onestore::page::Outline {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline)
                if outline.paragraphs.iter().any(|paragraph| {
                    matches!(&paragraph.content,
                        onestore::page::ParagraphContent::Text(run) if run.text.text() == text)
                }) =>
            {
                Some(outline)
            }
            _ => None,
        })
        .unwrap()
}

#[test]
fn a_right_to_left_page_opens_at_its_right_end_and_stores_edits_in_onenotes_frame() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, SOURCE.to_vec()).unwrap();
    let space: ExGuid = section.pages().unwrap()[0].0;
    let page = section.page(space).unwrap();
    assert!(page.rtl);
    let margin = page.margin_origin;
    let ink_right = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Ink(ink) => ink.bounds().map(|b| b[0] + b[2] + ink.layout.x.unwrap()),
            _ => None,
        })
        .fold(f32::NEG_INFINITY, f32::max);
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

    // The view opens at the right end of the content, the drawings at its right edge.
    let scroll = view.scroll();
    assert_eq!(view.viewport.origin[0], -scroll.max[0]);
    let right = view.viewport.document_point([800.0, 0.0])[0];
    assert!(
        ink_right <= right && right < ink_right + 12.0,
        "{ink_right} {right}"
    );
    assert_eq!(scroll.min[0], 0.0);
    // Resizing keeps the right edge, as OneNote's does.
    let _ = view.resized([600, 600]).unwrap();
    assert_eq!(view.viewport.document_point([600.0, 0.0])[0], right);
    let _ = view.resized([800, 600]).unwrap();
    assert_eq!(view.viewport.document_point([800.0, 0.0])[0], right);

    let mut at = 134_100_000_000_000_000;
    let mut store = |view: &mut PageView, section: &mut Section| {
        let ops = view.editor.take_ops().unwrap();
        assert!(!ops.is_empty());
        at += 10_000_000;
        let ops = ops.into_iter().map(|op| Op::Page { space, op }).collect();
        section.apply("Snowbound Test", &Edit { at, ops }).unwrap();
        section.page(space).unwrap()
    };

    // A click on blank page puts an outline there, on the grid anchored at the margin origin.
    let click = [240.0, 480.0];
    let point = view.viewport.document_point(click);
    let _ = view.pointer_moved(click).unwrap();
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_released().unwrap();
    let _ = view
        .insert_text("Typed on a right-to-left page".into())
        .unwrap();
    let page = store(&mut view, &mut section);
    let typed = outline(&page, "Typed on a right-to-left page")
        .layout
        .clone();
    let [x, y] = [typed.x.unwrap(), typed.y.unwrap()];
    for (stored, clicked, origin) in [(x, point[0], margin[0]), (y, point[1], margin[1])] {
        assert!(
            ((stored - origin) / 18.0).fract().abs() < 1e-3,
            "{stored} {origin}"
        );
        assert!((stored - clicked).abs() <= 18.0, "{stored} {clicked}");
    }
    assert!(x > margin[0]);

    // A stroke lands in page points where the pen went.
    let _ = view.set_tool(Tool::Pen(Pen::new(1.0, Some(0x1f4e79))));
    let path: Vec<[f32; 2]> = (0..=20)
        .map(|step| [300.0 + step as f32 * 5.0, 520.0 + step as f32 * 2.0])
        .collect();
    drag(&mut view, &path);
    let known: Vec<ExGuid> = page.objects.iter().map(PageObject::id).collect();
    let page = store(&mut view, &mut section);
    let [PageObject::Ink(ink)] = page
        .objects
        .iter()
        .filter(|object| !known.contains(&object.id()))
        .collect::<Vec<_>>()[..]
    else {
        panic!("the stroke is not one new drawing")
    };
    let offset = [ink.layout.x.unwrap_or(0.0), ink.layout.y.unwrap_or(0.0)];
    let first = ink.strokes[0].points[0];
    let expected = view.viewport.document_point(path[0]);
    for axis in 0..2 {
        assert!((first[axis] + offset[axis] - expected[axis]).abs() < 0.5);
    }
    let _ = view.set_tool(Tool::Select);

    // The header drags an outline two grid columns right, onto the grid anchored at the
    // margin origin.
    let before = outline(&page, "Fictitious positioned outline.")
        .layout
        .clone();
    let header = [
        before.x.unwrap() * view.viewport.scale + view.viewport.origin[0] + 40.0,
        before.y.unwrap() * view.viewport.scale + view.viewport.origin[1] - 8.0,
    ];
    let steps: Vec<[f32; 2]> = (0..=12)
        .map(|step| [header[0] + step as f32 * 4.0, header[1] + step as f32 * 2.0])
        .collect();
    drag(&mut view, &steps);
    let page = store(&mut view, &mut section);
    let after = outline(&page, "Fictitious positioned outline.")
        .layout
        .clone();
    assert_eq!(after.x.unwrap() - before.x.unwrap(), 36.0);
    assert!(after.y.unwrap() > before.y.unwrap());
    assert_eq!(((after.y.unwrap() - margin[1]) / 18.0).fract(), 0.0);

    section.seal().unwrap();
    let written = section.image();
    if let Some(directory) = std::env::var_os("SNOWBOUND_RTL_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
