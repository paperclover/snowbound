use canvas::gpu::{Renderer, Viewport, page::PageScene};
use canvas::{layout::TextEngine, page::Page};
use onestore::{RevisionIndex, Store, document::Document};
use std::{env, fs, sync::Arc, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1).peekable();
    let path = args
        .next()
        .ok_or("Expected a section snapshot, page title and new PNG path")?;
    let title = args.next().ok_or("Expected a page title")?;
    let destination = args.next().ok_or("Expected a new PNG path")?;
    let origin_y: f32 = if args
        .peek()
        .is_some_and(|value| value != "--substitute-font")
    {
        args.next().unwrap().parse()?
    } else {
        0.0
    };
    let mut engine = TextEngine::default();
    while let Some(option) = args.next() {
        if option != "--substitute-font" {
            return Err(format!("Unknown option: {option}").into());
        }
        let path = args
            .next()
            .ok_or("Provide a font file after --substitute-font.")?;
        engine.register_substitute(parley::fontique::Blob::new(Arc::new(fs::read(path)?)))?;
    }
    let page = {
        let bytes = fs::read(path)?;
        let store = Store::parse(&bytes)?;
        let index = RevisionIndex::parse(&store)?;
        Page::from_document(&Document::parse(&index)?, &title)?
    };
    let margin_origin = page.margin_origin;
    let object_count = page.objects.len();
    let scene = PageScene::new(page, &mut engine)?;
    let mut primitives = Vec::new();
    scene.append_primitives(&mut primitives, [0.0; 2])?;
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
    let size = [1386_u32, 759];
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Native page comparison"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let row_bytes = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Native page comparison readback"),
        size: u64::from(row_bytes) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut renderer = Renderer::new(device, queue, format);
    renderer
        .draw(
            &target.create_view(&Default::default()),
            Viewport {
                size,
                scale: 96.0 / 72.0,
                origin: [
                    (36.0 - margin_origin[0]) * (96.0 / 72.0),
                    (14.4 - margin_origin[1]) * (96.0 / 72.0) + origin_y,
                ],
            },
            &primitives,
        )
        .map_err(|e| format!("Page comparison rendering failed: {e:?}"))?;
    let mut encoder = renderer.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        target.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row_bytes),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    renderer.queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    buffer.map_async(wgpu::MapMode::Read, .., move |result| {
        let _ = sender.send(result);
    });
    renderer.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: Some(Duration::from_secs(10)),
    })?;
    receiver.recv_timeout(Duration::from_secs(10))??;
    let mapped = buffer.get_mapped_range(..)?;
    let pixels: Vec<_> = mapped
        .chunks_exact(row_bytes as usize)
        .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
        .collect();
    let mut encoder = png::Encoder::new(fs::File::create_new(destination)?, size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header()?.write_image_data(&pixels)?;
    eprintln!(
        "Rendered {} page objects at 100% / 96 DPI, origin Y {origin_y} px using {:?}",
        object_count,
        adapter.get_info()
    );
    Ok(())
}
