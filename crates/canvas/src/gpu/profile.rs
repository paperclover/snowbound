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
                scene.append_primitives(&mut primitives, [0.0; 2]).unwrap();
            } else {
                primitives.push(draw::Primitive::Text {
                    clip: None,
                    text: layout.as_ref().unwrap(),
                    origin: [36.0, 90.0],
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
