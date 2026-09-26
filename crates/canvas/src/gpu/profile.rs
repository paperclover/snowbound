use super::*;
use crate::layout::TextEngine;
use draw::Renderer;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use parley::fontique::Blob;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[test]
#[ignore = "manual release GPU timing and cache occupancy probe"]
fn renderer_cost() {
    let mut engine = TextEngine::default();
    if let Some(font) = std::env::var_os("CANVAS_TEST_SUBSTITUTE") {
        engine
            .register_substitute(Blob::new(Arc::new(std::fs::read(font).unwrap())))
            .unwrap();
    }
    let source = std::env::var_os("CANVAS_TEST_SECTION").map(|path| std::fs::read(path).unwrap());
    let title = std::env::var("CANVAS_TEST_PAGE").unwrap_or_default();
    let mut scene = source
        .as_ref()
        .map(|bytes| load_scene(bytes, &title, &mut engine));
    let layout = source.is_none().then(|| {
        engine
            .layout(
                &Paragraph::new(
                    "A paragraph with café, trees 🌳 and a few words.\n".repeat(2000),
                    Default::default(),
                ),
                360.0,
            )
            .unwrap()
    });
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("canvas_gpu_adapter\t{:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let size = [1386, 759];
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Canvas profile target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut renderer = Renderer::new(device, queue, format);
    let occupancy = renderer.occupancy();
    eprintln!(
        "canvas_gpu_allocated_bytes\tatlas\t{}\tvertex_buffer\t{}\ttarget\t{}",
        occupancy.atlas_bytes,
        occupancy.vertex_buffer_bytes,
        u64::from(size[0]) * u64::from(size[1]) * 4
    );
    for phase in [
        "warm",
        "pan",
        "zoom",
        "evict",
        "offscreen_x",
        "offscreen_y",
        "reopen",
    ] {
        if phase == "reopen" && source.is_none() {
            continue;
        }
        if phase.starts_with("offscreen") {
            renderer.clear_glyph_cache();
            renderer.clear_images();
        }
        for sample in 0..32 {
            if phase == "evict" {
                renderer.clear_glyph_cache();
                renderer.clear_images();
            }
            let mut viewport = Viewport {
                size,
                scale: 96.0 / 72.0,
                origin: [0.0; 2],
            };
            match phase {
                "pan" => viewport.origin[1] = -(sample as f32 * 63.25),
                "zoom" => viewport.scale *= [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0][sample % 8],
                "offscreen_x" => viewport.origin[0] = 1_000_000.0,
                "offscreen_y" => viewport.origin[1] = 1_000_000.0,
                "reopen" => {
                    scene = Some(load_scene(source.as_ref().unwrap(), &title, &mut engine));
                }
                _ => {}
            }
            let mut primitives = Vec::new();
            if let Some(scene) = &scene {
                scene
                    .append_primitives(&mut primitives, [0.0; 2], Paper::WHITE)
                    .unwrap();
            } else {
                primitives.push(draw::Primitive::Text {
                    clip: None,
                    text: layout.as_ref().unwrap(),
                    origin: [36.0, 90.0],
                    ink: Paper::WHITE.ink,
                });
            }
            let start = Instant::now();
            renderer
                .draw(&view, size, [1.0; 4], &[viewport.layer(&primitives)])
                .unwrap();
            let submitted = start.elapsed().as_nanos();
            renderer
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })
                .unwrap();
            let completed = start.elapsed().as_nanos();
            let occupancy = renderer.occupancy();
            assert!(occupancy.within_budget);
            eprintln!(
                "canvas_gpu_sample\t{phase}\t{sample}\t{submitted}\t{completed}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}",
                occupancy.glyphs,
                occupancy.atlas_texels,
                occupancy.images,
                occupancy.image_bytes,
                occupancy.vertices,
                occupancy.batches,
                occupancy.vertex_capacity_bytes,
                occupancy.batch_capacity_bytes,
                occupancy.glyph_capacity,
                occupancy.image_capacity
            );
        }
    }
}

fn load_scene(bytes: &[u8], title: &str, engine: &mut TextEngine) -> page::PageScene {
    let store = onestore::Store::parse(bytes).unwrap();
    let index = onestore::RevisionIndex::parse(&store).unwrap();
    page::PageScene::new(
        Page::from_document(&onestore::document::Document::parse(&index).unwrap(), title).unwrap(),
        engine,
    )
    .unwrap()
}

/// Frame cost of a large page as the app draws it: the view's primitives and the renderer's
/// submission, resting, scrolling, typing and pressing Enter. `CANVAS_TEST_SECTION` and
/// `CANVAS_TEST_PAGE` choose a stored page; otherwise one outline of 4000 paragraphs, which then
/// gains a picture and a table in its middle to type beside and in.
#[cfg(feature = "interaction")]
#[test]
#[ignore = "manual release frame timing probe"]
fn frame_cost() {
    use crate::document::{TextDocument, TextPosition};
    use crate::editor::CanvasEditor;
    use crate::interaction::{PageView, TextColors};
    use draw::edit::{Key, NamedKey};
    use onestore::page::{
        Image, ParagraphContent, Table, TableCell, TableColumn, TableRow, text::new_id,
    };
    const LINES: usize = 4000;
    let lines = || {
        (0..LINES).map(|line| {
            Paragraph::new(
                format!("Line {line}: a paragraph of notes with café and a few more words to wrap across the outline's width."),
                Default::default(),
            )
        })
    };
    let synthetic = std::env::var_os("CANVAS_TEST_SECTION").is_none();
    let mut engine = TextEngine::default();
    let (editor, scene) = match std::env::var_os("CANVAS_TEST_SECTION") {
        Some(path) => {
            let bytes = std::fs::read(path).unwrap();
            let store = onestore::Store::parse(&bytes).unwrap();
            let index = onestore::RevisionIndex::parse(&store).unwrap();
            let document = onestore::document::Document::parse(&index).unwrap();
            let title = std::env::var("CANVAS_TEST_PAGE").unwrap_or_default();
            let page = Page::from_document(&document, &title).unwrap();
            let (scene, editor) = page::PageScene::from_page(page, &mut engine).unwrap();
            (editor, Some((scene, [0.0; 2])))
        }
        None => {
            let document = TextDocument::new(lines().collect()).unwrap();
            let editor = CanvasEditor::new(&mut engine, document, 480.0).unwrap();
            (editor, None)
        }
    };
    let size = [2360, 1300];
    let mut view = PageView::new(editor, engine, scene, size, 2.0, Duration::from_millis(500));
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Frame profile target"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let target = target.create_view(&Default::default());
    let mut renderer = Renderer::new(device, queue, format);
    let colors = TextColors {
        caret: [0.0, 0.0, 0.0, 1.0],
        selection: [0.7, 0.8, 1.0, 1.0],
        paper: Paper::WHITE,
    };
    let _ = view.pointer_moved([400.0, 400.0]).unwrap();
    for phase in ["rest", "scroll", "type", "enter", "picture", "cell"] {
        match phase {
            "picture" if synthetic => {
                let mut nodes = TextDocument::new(lines().collect())
                    .unwrap()
                    .nodes()
                    .to_vec();
                let mut picture = nodes[0].clone();
                picture.id = new_id().unwrap();
                picture.content = ParagraphContent::Image(Image {
                    id: new_id().unwrap(),
                    layout: onestore::document::Layout {
                        max_width: Some(120.0),
                        max_height: Some(80.0),
                        ..Default::default()
                    },
                    size: None,
                    bytes: None,
                    alt: None,
                    background: false,
                });
                let cell = || TableCell {
                    id: new_id().unwrap(),
                    layout: Default::default(),
                    indents: vec![18.0, 0.0, 27.0],
                    shading: None,
                    paragraphs: vec![
                        crate::document::node(
                            Paragraph::new("Cell".into(), Default::default()),
                            Default::default(),
                        )
                        .unwrap(),
                    ],
                    unsupported: Vec::new(),
                };
                let mut table = nodes[0].clone();
                table.id = new_id().unwrap();
                table.content = ParagraphContent::Table(Table {
                    id: new_id().unwrap(),
                    columns: vec![
                        TableColumn {
                            width: 72.0,
                            locked: true,
                        };
                        2
                    ],
                    rows: (0..2)
                        .map(|_| TableRow {
                            id: new_id().unwrap(),
                            cells: vec![cell(), cell()],
                        })
                        .collect(),
                    borders: Some(true),
                    layout: Default::default(),
                    tags: Vec::new(),
                });
                // The picture precedes paragraph LINES / 2, which the table follows.
                nodes.insert(LINES / 2, picture);
                nodes.insert(LINES / 2 + 2, table);
                let document = TextDocument::from_nodes(nodes).unwrap();
                view.editor = CanvasEditor::new(&mut view.engine, document, 480.0).unwrap();
            }
            "picture" | "cell" if !synthetic => continue,
            _ => {}
        }
        let caret = match phase {
            "picture" => Some(LINES / 2),
            "cell" => Some(LINES / 2 + 1),
            _ => None,
        };
        if let Some(paragraph) = caret {
            view.editor
                .select(
                    [TextPosition {
                        paragraph,
                        offset: 0,
                    }; 2]
                        .into(),
                )
                .unwrap();
        }
        let mut times = [Vec::new(), Vec::new(), Vec::new()];
        for _ in 0..64 {
            let start = Instant::now();
            match phase {
                "scroll" => {
                    let _ = view.wheel([0.0, -120.0]).unwrap();
                }
                "enter" => {
                    let _ = view.key(&Key::Named(NamedKey::Enter), None).unwrap();
                }
                "type" | "picture" | "cell" => {
                    let _ = view.key(&Key::Named(NamedKey::Other), Some("a")).unwrap();
                }
                _ => {}
            }
            let input = start.elapsed();
            let start = Instant::now();
            view.update_pictures(colors.paper, std::task::Waker::noop());
            let primitives = view.primitives(colors).unwrap();
            let built = start.elapsed();
            let start = Instant::now();
            renderer
                .draw(&target, size, [1.0; 4], &[view.viewport.layer(&primitives)])
                .unwrap();
            let drawn = start.elapsed();
            drop(primitives);
            renderer
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: Some(Duration::from_secs(10)),
                })
                .unwrap();
            for (list, time) in times.iter_mut().zip([input, built, drawn]) {
                list.push(time);
            }
        }
        for (name, mut list) in ["input", "primitives", "draw"].into_iter().zip(times) {
            list.sort();
            eprintln!(
                "frame_cost\t{phase}\t{name}\tp50 {:.2} ms\tp99 {:.2} ms",
                list[list.len() / 2].as_secs_f64() * 1e3,
                list[list.len() * 99 / 100].as_secs_f64() * 1e3
            );
        }
    }
}
