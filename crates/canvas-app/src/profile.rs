use super::*;
use one_canvas::document::TextPosition;
use std::{hint::black_box, process::Command};

fn timed<T>(case: &str, phase: &str, sample: usize, operation: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = operation();
    eprintln!(
        "canvas_profile\t{case}\t{phase}\t{sample}\t{}",
        start.elapsed().as_nanos()
    );
    result
}

fn resident(case: &str, phase: &str, sample: usize) {
    let output = Command::new("/bin/ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .unwrap();
    assert!(output.status.success());
    let kib: u64 = std::str::from_utf8(&output.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    eprintln!("canvas_rss_kib\t{case}\t{phase}\t{sample}\t{kib}");
}

fn layout_ids(editor: &CanvasEditor) -> Vec<u64> {
    editor
        .outlines()
        .iter()
        .flat_map(|outline| outline.layouts().map(|(_, paragraph)| paragraph.text.id()))
        .collect()
}

fn editing_cost(
    case: &str,
    engine: &mut TextEngine,
    editor: &mut CanvasEditor,
    scene: Option<&(PageScene, [f32; 2])>,
) {
    let viewport = Viewport {
        size: [1000, 720],
        scale: 96.0 / 72.0,
        origin: [48.0; 2],
    };
    let initial: Vec<_> = timed(case, "opening_snapshot", 0, || {
        editor
            .outlines()
            .iter()
            .map(|outline| outline.document().clone())
            .collect()
    });
    let mut access = accessibility::Accessibility::default();
    let update = timed(case, "ax_first", 0, || {
        access.update(editor, viewport, "Profile", None).unwrap()
    });
    eprintln!("canvas_count\t{case}\tax_nodes\t0\t{}", update.nodes.len());
    drop(update);
    let count: usize = editor
        .outlines()
        .iter()
        .map(|outline| outline.document().nodes().len())
        .sum();
    eprintln!("canvas_count\t{case}\tparagraphs\t0\t{count}");
    for sample in 0..32 {
        let index = sample % editor.outlines().len();
        let id = editor.outlines()[index].id;
        editor.focus_outline(id).unwrap();
        let paragraph = if sample % 2 == 0 {
            0
        } else {
            editor.active_outline().layouts().last().unwrap().0
        };
        editor
            .select(
                [TextPosition {
                    paragraph,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        let original_layouts = layout_ids(editor);
        timed(case, "insert", sample, || {
            editor.insert(engine, "x").unwrap()
        });
        let edited_layouts = layout_ids(editor);
        let changed = original_layouts
            .iter()
            .zip(&edited_layouts)
            .filter(|(a, b)| a != b)
            .count();
        assert_eq!(changed, 1);
        eprintln!("canvas_count\t{case}\tchanged_layouts\t{sample}\t{changed}");
        let primitives = timed(case, "frame_primitives", sample, || {
            let primitives =
                page_primitives(editor, scene, None, None, true, viewport.scale, 1.0).unwrap();
            black_box(&primitives);
            primitives.len()
        });
        eprintln!("canvas_count\t{case}\tprimitives\t{sample}\t{primitives}");
        timed(case, "ax_insert", sample, || {
            black_box(access.update(editor, viewport, "Profile", None).unwrap());
        });
        assert_eq!(edited_layouts, layout_ids(editor));
        timed(case, "undo", sample, || {
            assert!(editor.undo(engine).unwrap())
        });
        timed(case, "ax_undo", sample, || {
            black_box(access.update(editor, viewport, "Profile", None).unwrap());
        });
        assert_eq!(editor.active_outline().document(), &initial[index]);
    }
    resident(case, "after_edits", 0);
}

#[test]
#[ignore = "manual release canvas pipeline timing probe"]
fn canvas_pipeline_cost() {
    if let Some(path) = std::env::var_os("CANVAS_TEST_SECTION") {
        let bytes = timed("native", "read", 0, || std::fs::read(path).unwrap());
        let title = std::env::var("CANVAS_TEST_PAGE").unwrap();
        let mut engine = timed("native", "engine", 0, TextEngine::default);
        if let Some(font) = std::env::var_os("CANVAS_TEST_SUBSTITUTE") {
            timed("native", "font", 0, || {
                engine
                    .register_substitute(parley::fontique::Blob::new(Arc::new(
                        std::fs::read(font).unwrap(),
                    )))
                    .unwrap()
            });
        }
        let cycles = std::env::var("CANVAS_PROFILE_CYCLES")
            .map(|value| value.parse::<usize>().unwrap())
            .unwrap_or(16);
        assert!(cycles > 0);
        for cycle in 0..cycles {
            let page = timed("native", "parse", cycle, || {
                let store = onestore::Store::parse(&bytes).unwrap();
                let index = onestore::RevisionIndex::parse(&store).unwrap();
                Page::from_document(
                    &onestore::document::Document::parse(&index).unwrap(),
                    &title,
                )
                .unwrap()
            });
            let (scene, mut editor) = timed("native", "scene", cycle, || {
                PageScene::from_page(page, &mut engine).unwrap()
            });
            let scene = (scene, [0.0; 2]);
            if cycle == 0 {
                editing_cost("native", &mut engine, &mut editor, Some(&scene));
            } else {
                black_box(
                    page_primitives(&editor, Some(&scene), None, None, false, 96.0 / 72.0, 1.0)
                        .unwrap(),
                );
            }
            resident("native", "open", cycle);
            drop(editor);
            drop(scene);
            resident("native", "closed", cycle);
        }
    } else {
        for count in [100, 1_000, 5_000] {
            let case = format!("synthetic_{count}");
            let mut engine = timed(&case, "engine", 0, TextEngine::default);
            let document = timed(&case, "document", 0, || {
                TextDocument::new(
                    (0..count)
                        .map(|_| {
                            Paragraph::new(
                                "A paragraph of plain text with several words.".into(),
                                Format::default(),
                            )
                        })
                        .collect(),
                )
                .unwrap()
            });
            let mut editor = timed(&case, "scene", 0, || {
                CanvasEditor::new(&mut engine, document, 480.0).unwrap()
            });
            editing_cost(&case, &mut engine, &mut editor, None);
            drop(editor);
            drop(engine);
            resident(&case, "closed", 0);
        }
    }
}
