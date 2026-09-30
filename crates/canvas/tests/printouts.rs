//! A file printout, whose pictures OneNote 2010 stores as pages of an XPS package with a PNG
//! rendering of each beside it (WebPictureContainer14), shows that rendering and edits like
//! any picture: its pages stay printouts when moved, and a page deleted comes back on undo
//! as the printout page it was (`corpus/printout`).

use canvas::{document::TextPosition, editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, Section, Store,
    op::{Edit, Op},
    page::{Image, Page, PageObject},
};

const PRINTOUTS: &[u8] = include_bytes!("../../../corpus/printout/native/notebook/Printouts.one");

fn pictures(page: &Page) -> Vec<&Image> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Image(image) => Some(image),
            _ => None,
        })
        .collect()
}

/// `SNOWBOUND_PRINTOUT_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn printouts_show_onenotes_rendering_and_move_as_pictures() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, PRINTOUTS.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Printout")
        .unwrap();
    let page = section.page(space).unwrap();
    let printed = pictures(&page);
    assert_eq!(printed.len(), 2);
    for picture in &printed {
        let (stored, shown) = (
            picture.bytes.as_ref().unwrap(),
            picture.display.as_ref().unwrap(),
        );
        // The stored picture is the XPS package; the shown one a PNG.
        assert!(stored.starts_with(b"PK\x03\x04"));
        assert!(shown.starts_with(b"\x89PNG"));
    }
    assert!(Iterator::eq(
        printed.iter().map(|picture| picture.bytes.as_ref()),
        [printed[0].bytes.as_ref(); 2]
    ));
    let (moved, layout) = (printed[1].id, printed[1].layout.clone());
    let origin = [layout.x.unwrap(), layout.y.unwrap() + 36.0];
    let size = [layout.max_width.unwrap(), layout.max_height.unwrap()];

    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    #[cfg(feature = "gpu")]
    {
        let (scene, editor) =
            canvas::gpu::page::PageScene::from_page(page.clone(), &mut engine).unwrap();
        assert_eq!(scene.read_only(Some(&editor)).count(), 0);
    }
    editor
        .place_image(&mut engine, moved, origin, size)
        .unwrap();
    let restored = printed[0].id;
    editor.remove_image(&mut engine, restored).unwrap();
    assert!(editor.undo(&mut engine).unwrap());
    let body = editor.outlines().iter().find(|o| !o.title).unwrap().id;
    editor.focus_outline(body).unwrap();
    let at = TextPosition {
        paragraph: 0,
        offset: 0,
    };
    editor.select([at; 2].into()).unwrap();
    editor.insert(&mut engine, "Printed ").unwrap();
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    let written = section.image();
    let reread = Section::open(&arena, written.clone()).unwrap();
    let page = reread.page(space).unwrap();
    let after = pictures(&page);
    assert_eq!(after.len(), 2);
    let moved = after.iter().find(|picture| picture.id == moved).unwrap();
    assert_eq!([moved.layout.x, moved.layout.y], origin.map(Some));
    assert!(moved.printout.is_some() && moved.display.is_some());
    // The page deleted and brought back by undo is the printout page it was: its package,
    // page number and the raster OneNote showed of it.
    let original = printed
        .iter()
        .find(|picture| picture.id == restored)
        .unwrap();
    let restored = after.iter().find(|picture| picture.id == restored).unwrap();
    assert_eq!(restored.printout, original.printout);
    assert_eq!(restored.bytes, original.bytes);
    assert_eq!(restored.display, original.display);
    if let Some(directory) = std::env::var_os("SNOWBOUND_PRINTOUT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("Printouts.one"), &written).unwrap();
        let file_id = Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("Printouts.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}

/// A copy of a printout's page keeps its pages printouts, as a page copied in OneNote does.
#[test]
fn a_copied_printout_page_keeps_its_printouts() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, PRINTOUTS.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Printout")
        .unwrap();
    let page = section.page(space).unwrap();
    let creation = onestore::PageCreation::new(None, Some("Copied"), "Author").unwrap();
    let edit = Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Section(onestore::op::SectionOp::Import {
            creation,
            page: page.copy().unwrap(),
        })],
    };
    section.apply("Author", &edit).unwrap();
    section.seal().unwrap();
    let (copied, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Copied")
        .unwrap();
    let copy = section.page(copied).unwrap();
    let shown = |page: &Page| -> Vec<_> {
        pictures(page)
            .into_iter()
            .map(|picture| {
                (
                    picture.printout.clone(),
                    picture.bytes.clone(),
                    picture.display.clone(),
                    picture.text.clone(),
                )
            })
            .collect()
    };
    assert_eq!(shown(&copy), shown(&page));
}
