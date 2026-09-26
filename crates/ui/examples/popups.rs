//! Renders each popup control open, in both themes, to `/tmp/ui-popups-{dark,light}.png`,
//! and frames of lists changing to `/tmp/ui-list-{keystroke,delete}-{dark,light}.png`: a
//! combo's list after `c` is typed into its filter, and a page list losing rows. Then times
//! a list of 200 000 rows.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};
use ui::{Anchor, Event, Id, List, Rows, Spec, Theme, Ui, fill, popup::Item, px, shell};
use winit::keyboard::{Key, NamedKey};

const PANEL: [f32; 2] = [420.0, 340.0];
const SCALE: f32 = 2.0;

macro_rules! art {
    ($path:literal) => {
        &[include_str!(concat!(
            "../../snowbound/assets/",
            $path,
            ".svg"
        ))]
    };
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut renderer = draw::Renderer::new(
        device.clone(),
        queue.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );
    for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
        let scenes: [fn(&mut Scene); 6] = [menu, fonts, colors, palette, tags, highlight];
        let uis: Vec<Ui> = scenes
            .iter()
            .map(|build| {
                let mut scene = Scene {
                    ui: Ui::new(theme.clone(), Duration::from_millis(500)),
                    start: Instant::now(),
                    frames: 0,
                };
                build(&mut scene);
                scene.ui
            })
            .collect();
        let panels = uis
            .iter()
            .map(|ui| paint(&device, &queue, &mut renderer, ui))
            .collect();
        let path = format!("/tmp/ui-popups-{name}.png");
        render(panels, 3, &path);
        println!("{path}");
        let changes = [
            (
                "keystroke",
                keystroke(&theme, &device, &queue, &mut renderer),
            ),
            ("delete", delete(&theme, &device, &queue, &mut renderer)),
        ];
        for (change, panels) in changes {
            let path = format!("/tmp/ui-list-{change}-{name}.png");
            render(panels, 5, &path);
            println!("{path}");
        }
    }
    measure();
}

/// Frames a change shows at: before it, then each of the 150 ms it takes.
const FRAMES: [u32; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 10];

/// A font combo's list before `c` is typed into its filter and at frames after.
fn keystroke(
    theme: &Theme,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut draw::Renderer,
) -> Vec<Vec<u8>> {
    let mut scene = Scene {
        ui: Ui::new(theme.clone(), Duration::from_millis(500)),
        start: Instant::now(),
        frames: 0,
    };
    let items = font_items();
    let id = Id::ROOT.child("fonts");
    let build = |ui: &mut Ui, [combo, _]: [Id; 2]| {
        let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
        ui::popup::menu(ui, id, anchor, &items, Some("Calibri"));
    };
    scene.open(id, Vec::new(), build);
    let mut panels = vec![paint(device, queue, renderer, &scene.ui)];
    for event in typed("c") {
        scene.ui.event(event);
    }
    for frame in 1..=10 {
        scene.frame(build);
        if FRAMES.contains(&frame) {
            panels.push(paint(device, queue, renderer, &scene.ui));
        }
    }
    panels
}

/// A page list before rows 3 to 5 and 9 are deleted and at frames after.
fn delete(
    theme: &Theme,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut draw::Renderer,
) -> Vec<Vec<u8>> {
    let mut scene = Scene {
        ui: Ui::new(theme.clone(), Duration::from_millis(500)),
        start: Instant::now(),
        frames: 0,
    };
    let names: Vec<String> = (0..30).map(|key| format!("Page {key}")).collect();
    let mut selected = Some(7);
    let mut build = |scene: &mut Scene, rows: &Keyed| {
        scene.frame(|ui, _| {
            let list = List {
                rows,
                row: 26.0,
                keys: &[],
                hover_selects: false,
            };
            let spec = Spec {
                size: [px(240.0), px(26.0 * 11.0)],
                fill: Some(ui.theme.panel),
                ..Spec::default()
            };
            let theme = ui.theme.clone();
            ui::list(
                ui,
                Id::ROOT.child("pages"),
                spec,
                list,
                &mut selected,
                |ui, row| {
                    ui.leaf(
                        "name",
                        Spec {
                            size: [fill(), fill()],
                            text: Some(&names[row.key as usize]),
                            fill: row.selected.then(|| theme.hover()),
                            radius: 4.0,
                            pad: [8.0, 0.0],
                            ..Spec::default()
                        },
                    );
                },
            );
        });
    };
    let all = Keyed::new((0..30).collect());
    for _ in 0..20 {
        build(&mut scene, &all);
    }
    let mut panels = vec![paint(device, queue, renderer, &scene.ui)];
    let kept = Keyed::new(
        (0..30)
            .filter(|key| !(3..=5).contains(key) && *key != 9)
            .collect(),
    );
    for frame in 1..=10 {
        build(&mut scene, &kept);
        if FRAMES.contains(&frame) {
            panels.push(paint(device, queue, renderer, &scene.ui));
        }
    }
    panels
}

/// Keys in order, and where each is.
struct Keyed {
    keys: Vec<u64>,
    at: HashMap<u64, usize>,
}

impl Keyed {
    fn new(keys: Vec<u64>) -> Self {
        let at = keys
            .iter()
            .enumerate()
            .map(|(index, key)| (*key, index))
            .collect();
        Self { keys, at }
    }
}

impl Rows for Keyed {
    fn count(&self) -> usize {
        self.keys.len()
    }

    fn key(&self, index: usize) -> u64 {
        self.keys[index]
    }

    fn find(&self, key: u64) -> Option<usize> {
        self.at.get(&key).copied()
    }
}

/// Prints how long a frame of a list takes to build and lay out: with 20 rows, with
/// 200 000, after a keystroke filters those, and while 10 000 arrive each frame.
fn measure() {
    let names: Vec<String> = (0..200_000u64)
        .map(|key| format!("Page {:06}", key.wrapping_mul(2_654_435_761) % 1_000_000))
        .collect();
    let mut ui = Ui::new(Theme::dark(), Duration::from_millis(500));
    let start = Instant::now();
    let mut frames = 0;
    let mut selected = Some(100_000);
    let mut frame = |ui: &mut Ui, rows: &Keyed, selected: &mut Option<u64>| {
        frames += 1;
        let began = Instant::now();
        ui.begin(
            [400.0, 600.0],
            2.0,
            start + Duration::from_millis(16) * frames,
        );
        let list = List {
            rows,
            row: 24.0,
            keys: &[],
            hover_selects: false,
        };
        let spec = Spec {
            size: [fill(), fill()],
            ..Spec::default()
        };
        ui::list(
            ui,
            Id::ROOT.child("pages"),
            spec,
            list,
            selected,
            |ui, row| {
                let text = &names[row.key as usize];
                ui.leaf(
                    "name",
                    Spec {
                        size: [fill(), fill()],
                        text: Some(text),
                        fill: row.selected.then_some([0.2, 0.3, 0.4, 1.0]),
                        pad: [8.0, 0.0],
                        ..Spec::default()
                    },
                );
            },
        );
        ui.end();
        drop(ui.layers());
        began.elapsed()
    };
    let average = |times: Vec<Duration>| times.iter().sum::<Duration>() / times.len() as u32;
    for count in [20, 200_000] {
        let rows = Keyed::new((0..count).collect());
        let times: Vec<_> = (0..60)
            .map(|_| frame(&mut ui, &rows, &mut selected))
            .collect();
        println!("{count} rows: {:?} a frame", average(times[30..].to_vec()));
    }
    let filtering = Instant::now();
    let rows = Keyed::new(
        (0..200_000)
            .filter(|key| names[*key as usize].contains('7'))
            .collect(),
    );
    let filtering = filtering.elapsed();
    let first = frame(&mut ui, &rows, &mut selected);
    let times: Vec<_> = (0..12)
        .map(|_| frame(&mut ui, &rows, &mut selected))
        .collect();
    println!(
        "a keystroke filtering 200 000 to {}: {filtering:?} to filter, then {first:?} and {:?} a frame as rows ease",
        rows.count(),
        average(times)
    );
    let mut keys: Vec<u64> = (0..100_000).collect();
    let (mut list, mut rebuild) = (Vec::new(), Vec::new());
    for wave in 0..10u64 {
        let rebuilding = Instant::now();
        // Arrivals interleave with the rows already listed, above and below the view.
        let arriving = (0..10_000).map(|index| 100_000 + wave * 10_000 + index);
        keys = keys
            .chunks(10)
            .zip(arriving)
            .flat_map(|(chunk, key)| chunk.iter().copied().chain([key]))
            .collect();
        let rows = Keyed::new(keys.clone());
        rebuild.push(rebuilding.elapsed());
        list.push(frame(&mut ui, &rows, &mut selected));
    }
    println!(
        "10 000 arriving a frame, up to 200 000: {:?} a frame for the list, {:?} to rebuild the rows",
        average(list),
        average(rebuild)
    );
}

struct Scene {
    ui: Ui,
    start: Instant,
    frames: u32,
}

impl Scene {
    /// Builds a frame over the panel's backdrop and toolbar, handing `build` the ids of the
    /// combo and the split button's arrow.
    fn frame(&mut self, build: impl FnOnce(&mut Ui, [Id; 2])) {
        self.frames += 1;
        let now = self.start + Duration::from_millis(16) * self.frames;
        let ui = &mut self.ui;
        let theme = ui.theme.clone();
        ui.begin(PANEL, SCALE, now);
        ui.open(
            "backdrop",
            Spec {
                size: [fill(), fill()],
                fill: Some(theme.base),
                ..Spec::default()
            },
        );
        ui.open(
            "toolbar",
            Spec {
                size: [fill(), px(30.0)],
                fill: Some(theme.strip),
                pad: [8.0, 4.0],
                gap: 6.0,
                ..Spec::default()
            },
        );
        let combo = ui.id("font");
        shell::combo(ui, "font", "Calibri", 120.0);
        let split = ui.id("color");
        shell::split_button(
            ui,
            "color",
            art!("icons/font-color"),
            Some(draw::srgb(0xe8, 0x3a, 0x30)),
        );
        ui.close();
        ui.close();
        build(ui, [combo, split.child("menu")]);
        ui.end();
    }

    /// Opens `id` and runs frames until it has faded in, feeding `events` once it shows.
    fn open(&mut self, id: Id, events: Vec<Event>, build: impl Fn(&mut Ui, [Id; 2])) {
        self.frame(|_, _| {});
        self.frame(|ui, ids| {
            ui.open_popup(id);
            build(ui, ids);
        });
        for event in events {
            self.ui.event(event);
        }
        for _ in 0..30 {
            self.frame(&build);
        }
    }
}

fn below(ui: &Ui, id: Id) -> Anchor {
    Anchor::Below(ui.rect(id).unwrap_or_default())
}

fn key(named: NamedKey) -> Event {
    Event::Key {
        key: Key::Named(named),
        text: None,
    }
}

fn typed(text: &str) -> Vec<Event> {
    text.chars()
        .map(|character| Event::Key {
            key: Key::Character(character.to_string().into()),
            text: Some(character.to_string()),
        })
        .collect()
}

fn menu(scene: &mut Scene) {
    let items = [
        Item {
            text: "Cut",
            shortcut: "⌘X",
            ..Item::default()
        },
        Item {
            text: "Copy",
            shortcut: "⌘C",
            ..Item::default()
        },
        Item {
            text: "Paste",
            shortcut: "⌘V",
            ..Item::default()
        },
        Item {
            text: "Paste as plain text",
            shortcut: "⌥⇧⌘V",
            disabled: true,
            ..Item::default()
        },
        Item {
            text: "Bold",
            icon: Some(art!("icons/bold")),
            shortcut: "⌘B",
            checked: true,
            separated: true,
            ..Item::default()
        },
        Item {
            text: "Italic",
            icon: Some(art!("icons/italic")),
            shortcut: "⌘I",
            ..Item::default()
        },
        Item {
            text: "Link…",
            icon: Some(art!("icons/link")),
            shortcut: "⌘K",
            ..Item::default()
        },
        Item {
            text: "Select all",
            shortcut: "⌘A",
            separated: true,
            ..Item::default()
        },
    ];
    let id = Id::ROOT.child("context");
    let events = vec![key(NamedKey::ArrowDown), key(NamedKey::ArrowDown)];
    scene.open(id, events, |ui, _| {
        ui::popup::menu(ui, id, Anchor::Point([140.0, 70.0]), &items, None);
    });
}

const FONTS: &[&str] = &[
    "Aptos",
    "Arial",
    "Arial Black",
    "Bahnschrift",
    "Calibri",
    "Cambria",
    "Candara",
    "Comic Sans MS",
    "Consolas",
    "Constantia",
    "Corbel",
    "Courier New",
    "Georgia",
    "Segoe UI",
    "Tahoma",
    "Times New Roman",
    "Trebuchet MS",
    "Verdana",
];

fn font_items() -> Vec<Item<'static>> {
    FONTS
        .iter()
        .map(|text| Item {
            text,
            checked: *text == "Calibri",
            ..Item::default()
        })
        .collect()
}

fn fonts(scene: &mut Scene) {
    let items = font_items();
    let id = Id::ROOT.child("fonts");
    scene.open(id, typed("ca"), |ui, [combo, _]| {
        let anchor = Anchor::Over(ui.rect(combo).unwrap_or_default());
        ui::popup::menu(ui, id, anchor, &items, Some("Calibri"));
    });
}

fn office_colors() -> Vec<[f32; 4]> {
    let theme = [
        (0xff, 0xff, 0xff),
        (0x00, 0x00, 0x00),
        (0xee, 0xec, 0xe1),
        (0x1f, 0x49, 0x7d),
        (0x4f, 0x81, 0xbd),
        (0xc0, 0x50, 0x4d),
        (0x9b, 0xbb, 0x59),
        (0x80, 0x64, 0xa2),
        (0x4b, 0xac, 0xc6),
        (0xf7, 0x96, 0x46),
    ]
    .map(|(r, g, b)| draw::srgb(r, g, b));
    let standard = [
        (0xc0, 0x00, 0x00),
        (0xff, 0x00, 0x00),
        (0xff, 0xc0, 0x00),
        (0xff, 0xff, 0x00),
        (0x92, 0xd0, 0x50),
        (0x00, 0xb0, 0x50),
        (0x00, 0xb0, 0xf0),
        (0x00, 0x70, 0xc0),
        (0x00, 0x20, 0x60),
        (0x70, 0x30, 0xa0),
    ]
    .map(|(r, g, b)| draw::srgb(r, g, b));
    let mut colors = theme.to_vec();
    for step in [0.15, 0.35, 0.6, -0.3, -0.6] {
        colors.extend(theme.iter().map(|color| {
            let toward = if step > 0.0 {
                [1.0; 4]
            } else {
                [0.0, 0.0, 0.0, 1.0]
            };
            ui::mix(*color, toward, f32::abs(step))
        }));
    }
    colors.extend(standard);
    colors
}

fn colors(scene: &mut Scene) {
    let swatches = office_colors();
    let id = Id::ROOT.child("font color");
    scene.open(id, Vec::new(), |ui, [_, split]| {
        let anchor = below(ui, split);
        ui::popup::colors(ui, id, anchor, "Automatic", &swatches, 10);
    });
    let cell = scene.ui.rect(id.child(("cell", 15_usize))).unwrap();
    scene
        .ui
        .event(Event::PointerMoved([cell[0] + 5.0, cell[1] + 5.0]));
    for _ in 0..30 {
        scene.frame(|ui, [_, split]| {
            let anchor = below(ui, split);
            ui::popup::colors(ui, id, anchor, "Automatic", &swatches, 10);
        });
    }
}

fn highlight(scene: &mut Scene) {
    let swatches = [
        (0xff, 0xff, 0x00),
        (0x00, 0xff, 0x00),
        (0x00, 0xff, 0xff),
        (0xff, 0x00, 0xff),
        (0x00, 0x00, 0xff),
        (0xff, 0x00, 0x00),
        (0x00, 0x00, 0x80),
        (0x00, 0x80, 0x80),
        (0x00, 0x80, 0x00),
        (0x80, 0x00, 0x80),
        (0x80, 0x00, 0x00),
        (0x80, 0x80, 0x00),
        (0x80, 0x80, 0x80),
        (0xc0, 0xc0, 0xc0),
        (0x00, 0x00, 0x00),
    ]
    .map(|(r, g, b)| draw::srgb(r, g, b));
    let id = Id::ROOT.child("highlight");
    scene.open(id, vec![key(NamedKey::ArrowDown)], |ui, [_, split]| {
        let anchor = below(ui, split);
        ui::popup::colors(ui, id, anchor, "No colour", &swatches, 5);
    });
}

fn palette(scene: &mut Scene) {
    let commands = [
        ("New page", "⌘N"),
        ("New section", "⌘T"),
        ("New notebook", ""),
        ("Open notebook…", "⌘O"),
        ("Go to page…", "⌘G"),
        ("Insert table", "⌥⌘T"),
        ("Insert date", "⇧⌘D"),
        ("Numbered list", "⌘/"),
        ("Bulleted list", "⌘."),
        ("Toggle page list", ""),
        ("Zoom in", "⌘="),
        ("Zoom out", "⌘-"),
    ]
    .map(|(text, shortcut)| Item {
        text,
        shortcut,
        ..Item::default()
    });
    let id = Id::ROOT.child("palette");
    scene.open(id, typed("ne"), |ui, _| {
        ui::popup::palette(ui, id, &commands);
    });
}

fn tags(scene: &mut Scene) {
    let tags = [
        ("To Do", None::<&'static [&'static str]>, "⌘1"),
        ("Important", Some(art!("tags/star")), "⌘2"),
        ("Question", None, "⌘3"),
        ("Remember for later", Some(art!("tags/remember")), "⌘4"),
        ("Definition", Some(art!("tags/definition")), "⌘5"),
        ("Highlight", Some(art!("tags/highlight")), "⌘6"),
        ("Contact", Some(art!("tags/contact")), "⌘7"),
        ("Address", Some(art!("tags/address")), "⌘8"),
        ("Phone number", Some(art!("tags/phone")), "⌘9"),
        ("Web site to visit", None, ""),
        ("Idea", None, ""),
        ("Password", None, ""),
        ("Critical", None, ""),
        ("Project A", None, ""),
        ("Project B", None, ""),
    ]
    .map(|(text, icon, shortcut)| Item {
        text,
        icon,
        colored: true,
        shortcut,
        ..Item::default()
    });
    let id = Id::ROOT.child("tags");
    scene.open(id, Vec::new(), |ui, [combo, _]| {
        let rect = ui.rect(combo).unwrap_or_default();
        let anchor = Anchor::Below([rect[0] + 200.0, rect[1], rect[0] + 222.0, rect[3]]);
        ui::popup::menu(ui, id, anchor, &tags, Some("Filter tags"));
    });
    let row = scene.ui.rect(id.child("rows").child(3_usize)).unwrap();
    scene
        .ui
        .event(Event::PointerMoved([row[0] + 40.0, row[1] + 8.0]));
    for _ in 0..3 {
        scene.frame(|ui, [combo, _]| {
            let rect = ui.rect(combo).unwrap_or_default();
            let anchor = Anchor::Below([rect[0] + 200.0, rect[1], rect[0] + 222.0, rect[3]]);
            ui::popup::menu(ui, id, anchor, &tags, Some("Filter tags"));
        });
    }
}

/// Lays out panels of `PANEL` in rows of `columns` and writes them to `path`.
fn render(panels: Vec<Vec<u8>>, columns: usize, path: &str) {
    let panel = PANEL.map(|length| (length * SCALE) as u32);
    let gap = 16;
    let rows = panels.len().div_ceil(columns) as u32;
    let size = [
        (panel[0] + gap) * columns as u32 + gap,
        (panel[1] + gap) * rows + gap,
    ];
    let mut image = vec![0x80; (size[0] * size[1] * 4) as usize];
    for (index, pixels) in panels.iter().enumerate() {
        let [left, top] = [
            gap + (index % columns) as u32 * (panel[0] + gap),
            gap + (index / columns) as u32 * (panel[1] + gap),
        ];
        for (y, line) in pixels.chunks((panel[0] * 4) as usize).enumerate() {
            let start = (((top + y as u32) * size[0] + left) * 4) as usize;
            image[start..start + line.len()].copy_from_slice(line);
        }
    }
    let file = std::fs::File::create(path).unwrap();
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&image)
        .unwrap();
}

/// Paints `ui`'s frame, each on its own so the atlas holds one popup as the app's would,
/// and reads it back.
fn paint(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    renderer: &mut draw::Renderer,
    ui: &Ui,
) -> Vec<u8> {
    let size = PANEL.map(|length| (length * SCALE) as u32);
    let interface = ui.layers();
    let layers: Vec<_> = interface
        .iter()
        .filter_map(|layer| match layer {
            ui::Layer::Primitives { clip, primitives } => Some(draw::Layer {
                scale: SCALE,
                origin: [0.0; 2],
                clip: clip.map(|clip| clip.map(|value| value * SCALE)),
                primitives,
            }),
            ui::Layer::Custom { .. } => None,
        })
        .collect();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Popups"),
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
    renderer
        .draw(
            &texture.create_view(&Default::default()),
            size,
            ui.theme.base,
            &layers,
        )
        .unwrap();
    let row = (size[0] * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Popups readback"),
        size: u64::from(row) * u64::from(size[1]),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(size[1]),
            },
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    mapped
        .chunks(row as usize)
        .flat_map(|line| &line[..size[0] as usize * 4])
        .copied()
        .collect()
}
