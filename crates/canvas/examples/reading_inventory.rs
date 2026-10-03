//! How many pages a reading view reflows into a phone's column, across the active pages of
//! every distinct section under the given directories, as Markdown; with `--json FILE`, one
//! line per page with its blocks in reading order. `--render SECTION INDEX PREFIX` draws page
//! INDEX of SECTION whole (`PREFIX-page.png`) and reflowed (`PREFIX-column.png`).
//!
//! `cargo run --release -p canvas --features gpu --example reading_inventory -- [--column 230] corpus`

use canvas::{
    gpu::{Paper, Viewport, page::PageScene},
    layout::TextEngine,
    reading::{self, Kept, Kind, Refusal, Verdict},
};
use draw::Renderer;
use onestore::{Arena, Section, page::Page};
use std::{
    collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher},
    fmt::Write as _,
    hash::{Hash, Hasher},
    io::Write as _,
    path::{Path, PathBuf},
    time::Duration,
};

/// Host points per page point at the iOS app's 100%: OneNote's 96 dpi enlarged 15%.
const POINT: f32 = 96.0 / 72.0 * 1.15;

fn sections(directory: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sections(&path, out);
        } else if path.extension().is_some_and(|e| e == "one") {
            out.push(path);
        }
    }
}

fn label(verdict: &Verdict) -> String {
    match verdict {
        Verdict::Fits => "fits".into(),
        Verdict::Reflows => "reflows".into(),
        Verdict::Partial(kept) => format!(
            "partial: {}",
            kept.iter()
                .map(|k| kept_name(*k))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Verdict::Refused(refusal) => format!(
            "refused: {}",
            match refusal {
                Refusal::RightToLeft => "right to left",
                Refusal::Drawn => "mostly ink",
                Refusal::Arranged => "mostly overlapping",
            }
        ),
    }
}

fn kept_name(kept: Kept) -> &'static str {
    match kept {
        Kept::Asides => "asides",
        Kept::SideBySide => "side by side",
        Kept::Group => "overlap kept whole",
        Kept::Wide => "wider than the column",
    }
}

fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "text",
        Kind::Picture => "picture",
        Kind::File => "file",
        Kind::Drawing => "drawing",
        Kind::Group => "group",
    }
}

fn open(path: &Path) -> Option<Vec<(String, Page)>> {
    let image = std::fs::read(path).ok()?;
    let arena = Arena::default();
    let mut section = Section::open(&arena, image).ok()?;
    let pages = section.pages().ok()?;
    Some(
        pages
            .into_iter()
            .filter_map(|(space, title, _)| section.page(space).ok().map(|page| (title, page)))
            .collect(),
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut column = 230.0;
    let mut json = None;
    if let Some(at) = args.iter().position(|a| a == "--column") {
        column = args[at + 1].parse()?;
        args.drain(at..at + 2);
    }
    if let Some(at) = args.iter().position(|a| a == "--json") {
        json = Some(std::fs::File::create(&args[at + 1])?);
        args.drain(at..at + 2);
    }
    let mut engine = TextEngine::default();
    if args.first().is_some_and(|a| a == "--render") {
        let [_, section, index, prefix] = &args[..] else {
            return Err("--render SECTION INDEX PREFIX".into());
        };
        let pages = open(Path::new(section)).ok_or("section does not open")?;
        let (_, page) = pages
            .into_iter()
            .nth(index.parse()?)
            .ok_or("no such page")?;
        let read = reading::read(&page, column, &mut engine)?;
        let mut gpu = Gpu::new()?;
        gpu.whole(&page, &mut engine, &format!("{prefix}-page.png"))?;
        if let Some(reflowed) = &read.page {
            gpu.column(
                reflowed,
                column,
                &mut engine,
                &format!("{prefix}-column.png"),
            )?;
        }
        println!("{}", label(&read.verdict));
        return Ok(());
    }
    let mut seen = BTreeSet::new();
    let mut distinct = BTreeSet::new();
    let mut verdicts: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let (mut pages, mut failed) = (0, 0);
    for root in &args {
        let mut paths = Vec::new();
        sections(Path::new(root), &mut paths);
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let mut hasher = DefaultHasher::new();
            bytes.hash(&mut hasher);
            if !seen.insert(hasher.finish()) {
                continue;
            }
            let Some(section) = open(&path) else {
                continue;
            };
            for (index, (title, page)) in section.into_iter().enumerate() {
                // Fixtures repeat pages across their runs; each page counts once.
                let mut hasher = DefaultHasher::new();
                serde_json::to_string(&page)?.hash(&mut hasher);
                if !distinct.insert(hasher.finish()) {
                    continue;
                }
                pages += 1;
                let place = format!("{} #{index} — {title}", path.display());
                let read = match reading::read(&page, column, &mut engine) {
                    Ok(read) => read,
                    Err(_) => {
                        failed += 1;
                        continue;
                    }
                };
                let verdict = label(&read.verdict);
                if let Some(json) = &mut json {
                    let blocks: Vec<_> = read
                        .blocks
                        .iter()
                        .map(|b| serde_json::json!({"kind": kind_name(b.kind), "members": b.members.len(), "bounds": b.bounds}))
                        .collect();
                    let line = serde_json::json!({
                        "section": path.display().to_string(), "index": index, "title": title,
                        "verdict": verdict, "offered": read.verdict.offered(), "blocks": blocks,
                    });
                    writeln!(json, "{line}")?;
                }
                verdicts.entry(verdict).or_default().push(place);
            }
        }
    }
    let mut out = String::new();
    writeln!(out, "# Reading view eligibility\n")?;
    writeln!(
        out,
        "{} distinct sections, {pages} distinct pages ({failed} failed to lay out), column {column} pt ({:.0} pt on screen at 100%).\n",
        seen.len(),
        column * POINT
    )?;
    writeln!(
        out,
        "| Verdict | Pages | Share | Examples |\n| --- | ---: | ---: | --- |"
    )?;
    let mut rows: Vec<_> = verdicts.iter().collect();
    rows.sort_by_key(|row| std::cmp::Reverse(row.1.len()));
    for (verdict, places) in rows {
        let examples: Vec<String> = places.iter().take(3).map(|p| format!("`{p}`")).collect();
        writeln!(
            out,
            "| {verdict} | {} | {:.1}% | {} |",
            places.len(),
            100.0 * places.len() as f64 / pages.max(1) as f64,
            examples.join("<br>").replace('|', "\\|")
        )?;
    }
    print!("{out}");
    Ok(())
}

struct Gpu {
    renderer: Renderer,
}

impl Gpu {
    fn new() -> Result<Self, Box<dyn std::error::Error>> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&Default::default()))?;
        Ok(Self {
            renderer: Renderer::new(device, queue, wgpu::TextureFormat::Rgba8UnormSrgb),
        })
    }

    /// The whole page, 804 pixels across: a phone's width at 2x.
    fn whole(
        &mut self,
        page: &Page,
        engine: &mut TextEngine,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (scene, bounds) = scene(page, engine)?;
        let [x0, y0, x1, y1] = bounds;
        let scale = 780.0 / (x1 - x0);
        let height = (((y1 - y0) * scale) as u32 + 24).min(4000);
        self.draw(
            scene,
            Viewport {
                size: [804, height],
                scale,
                origin: [12.0 - x0 * scale, 12.0 - y0 * scale],
            },
            path,
        )
    }

    /// The reflowed page as the phone shows it at 100%, 34 points of tag room on its left.
    fn column(
        &mut self,
        page: &Page,
        column: f32,
        engine: &mut TextEngine,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let (scene, [x0, y0, _, y1]) = scene(page, engine)?;
        let scale = 2.0 * POINT;
        let _ = column;
        let height = (((y1 - y0) * scale) as u32 + 48).min(6000);
        self.draw(
            scene,
            Viewport {
                size: [804, height],
                scale,
                origin: [68.0 - x0 * scale, 24.0 - y0 * scale],
            },
            path,
        )
    }

    fn draw(
        &mut self,
        mut scene: PageScene,
        viewport: Viewport,
        path: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        scene.settle(None, viewport.scale, Paper::WHITE);
        let mut primitives = Vec::new();
        scene.append_primitives(&mut primitives, [0.0; 2], Paper::WHITE)?;
        let size = viewport.size;
        let device = self.renderer.device().clone();
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: size[0],
                height: size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let row_bytes = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(row_bytes) * u64::from(size[1]),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        self.renderer
            .draw(
                &target.create_view(&Default::default()).into(),
                size,
                [1.0; 4],
                &[viewport.layer(&primitives)],
            )
            .map_err(|e| format!("{e:?}"))?;
        let mut encoder = device.create_command_encoder(&Default::default());
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
        self.renderer.queue().submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            let _ = sender.send(result);
        });
        device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(10)),
        })?;
        receiver.recv_timeout(Duration::from_secs(10))??;
        let mapped = buffer.get_mapped_range(..)?;
        let pixels: Vec<u8> = mapped
            .chunks_exact(row_bytes as usize)
            .flat_map(|row| row[..size[0] as usize * 4].iter().copied())
            .collect();
        let mut encoder = png::Encoder::new(std::fs::File::create(path)?, size[0], size[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        Ok(())
    }
}

/// The page's scene, pictures decoded, and its content's bounds with the title.
fn scene(
    page: &Page,
    engine: &mut TextEngine,
) -> Result<(PageScene, [f32; 4]), Box<dyn std::error::Error>> {
    let editor = canvas::editor::CanvasEditor::from_page(page.clone(), engine)?;
    let mut bounds = [
        f32::INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NEG_INFINITY,
    ];
    for outline in editor.outlines() {
        let b = outline.bounds();
        bounds = [
            bounds[0].min(b.x0 as f32),
            bounds[1].min(b.y0 as f32),
            bounds[2].max(b.x1 as f32),
            bounds[3].max(b.y1 as f32),
        ];
    }
    for block in reading::read(page, f32::INFINITY, engine)?.blocks {
        let b = block.bounds;
        bounds = [
            bounds[0].min(b[0]),
            bounds[1].min(b[1]),
            bounds[2].max(b[2]),
            bounds[3].max(b[3]),
        ];
    }
    Ok((PageScene::new(page.clone(), engine)?, bounds))
}
