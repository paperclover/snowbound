//! `reference OUT.png` paints the render-probe scene through `draw` over wgpu,
//! `reference --text SCALE OUT.png` the text-probe scene; `reference --compare A.png B.png
//! DIFF.png` reports how two renders differ and writes their difference, amplified.
#[path = "../../probe/src/scene.rs"]
mod scene;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [flag, a, b, diff] if flag == "--compare" => compare(a, b, diff),
        [flag, scale, out] if flag == "--text" => {
            let scale: f32 = scale.parse().unwrap();
            let ui = scene::interface(scale);
            let layers = ui.layers();
            let size = scene::TEXT_SIZE.map(|side| (side as f32 * scale) as u32);
            paint(out, size, [1.0; 4], &scene::interface_layers(&layers, scale));
        }
        [out] => {
            let checker = scene::checker();
            let primitives = scene::primitives(&checker);
            paint(out, scene::SIZE, scene::CLEAR, &[scene::layer(&primitives)]);
        }
        _ => panic!("usage: reference [--text SCALE] OUT.png | reference --compare A.png B.png DIFF.png"),
    }
}

fn read(path: &str) -> ([u32; 2], Vec<u8>) {
    let mut decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgba, "{path}");
    ([info.width, info.height], pixels)
}

fn compare(a: &str, b: &str, diff: &str) {
    let (size, a) = read(a);
    let (other, b) = read(b);
    assert_eq!(size, other, "sizes differ");
    let delta: Vec<u8> = a.iter().zip(&b).map(|(a, b)| a.abs_diff(*b)).collect();
    let worst = delta.iter().max().unwrap();
    let off = delta.chunks(4).filter(|p| p.iter().any(|v| *v > 2)).count();
    println!("max channel difference {worst}; {off} of {} pixels differ by more than 2", delta.len() / 4);
    let shown: Vec<u8> = delta
        .chunks(4)
        .flat_map(|p| [p[0], p[1], p[2]].map(|v| v.saturating_mul(16)).into_iter().chain([255]))
        .collect();
    write(diff, size, &shown);
}

fn write(path: &str, [width, height]: [u32; 2], pixels: &[u8]) {
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.write_header().unwrap().write_image_data(pixels).unwrap();
}

fn paint(out: &str, size: [u32; 2], clear: [f32; 4], layers: &[draw::Layer<'_>]) {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let format = wgpu::TextureFormat::Rgba8UnormSrgb;
    let mut renderer = draw::Renderer::new(device.clone(), queue.clone(), format);
    let [width, height] = size;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    renderer
        .draw(&texture.create_view(&Default::default()), size, clear, layers)
        .unwrap();
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(row * height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    let pixels: Vec<u8> = mapped.chunks(row as usize).flat_map(|r| &r[..width as usize * 4]).copied().collect();
    write(out, size, &pixels);
}
