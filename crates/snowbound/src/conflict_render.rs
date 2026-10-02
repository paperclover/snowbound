//! The conflict page and page version views drawn offscreen, from the conflict-page and
//! page-versions corpus rows: the page with its information bar, its conflict pages or
//! versions listed beneath it, and one open. `SNOWBOUND_CONFLICT_RENDER` names a directory
//! for the PNGs.

use super::*;

const SIZE: [f32; 2] = [980.0, 520.0];
const SCALE: f32 = 2.0;

/// One view laid out: the bar and the page on the left, the page list on the right; `menu`
/// opens the conflict page's menu.
fn layout(ui: &mut Ui, session: &Session, menu: bool) -> Vec<Command> {
    let theme = ui.theme.clone();
    let section = theme.section(section_color(None));
    ui.begin(SIZE, SCALE, Instant::now());
    if menu {
        ui.open_popup(Id::ROOT.child("conflict-menu"));
    }
    ui.open(
        "frame",
        Spec {
            size: [fill(), fill()],
            fill: Some(section.frame[0]),
            gradient: Some(section.frame[1]),
            pad: [FRAME, FRAME],
            ..Spec::default()
        },
    );
    ui.open(
        "column",
        Spec {
            axis: Axis::Y,
            size: [fill(), fill()],
            ..Spec::default()
        },
    );
    let mut commands: Vec<Command> = session
        .bar()
        .and_then(|bar| conflict_bar(ui, bar, &["synthetic", "Songs"], [false, true]))
        .into_iter()
        .collect();
    ui.open_as(
        page(),
        Spec {
            flags: Flags::CUSTOM,
            size: [fill(), fill()],
            fill: Some(theme.paper),
            ..Spec::default()
        },
    );
    ui.close();
    ui.close();
    ui.open(
        "panel",
        Spec {
            axis: Axis::Y,
            size: [px(PAGE_LIST), fill()],
            ..Spec::default()
        },
    );
    let rows = page_rows(
        ui,
        &theme,
        &section,
        session,
        [&HashSet::new(); 2],
        &HashMap::new(),
        (0.0, false),
        None,
        None,
    );
    commands.extend(rows.clicked.map(Command::OpenPage));
    ui.close();
    ui.close();
    ui.end();
    commands
}

/// The view's pixels, RGBA: the interface with `page` drawn in its box.
fn paint(
    ui: &Ui,
    page: Page,
    renderer: &mut draw::Renderer,
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> Vec<u8> {
    let size = SIZE.map(|side| (side * SCALE) as u32);
    let corner = ui.rect(super::page()).unwrap();
    let box_size = [
        ((corner[2] - corner[0]) * SCALE) as u32,
        ((corner[3] - corner[1]) * SCALE) as u32,
    ];
    let mut engine = TextEngine::default();
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        box_size,
        SCALE,
        std::time::Duration::from_millis(500),
    );
    let paper = canvas::gpu::Paper {
        color: ui.theme.paper,
        ink: ui.theme.paper_ink,
    };
    let waker = std::task::Waker::noop();
    let deadline = Instant::now() + std::time::Duration::from_secs(10);
    let mut scene = view.scene.take().unwrap();
    while !view.prepare(&mut scene, &view.editor, paper, waker) && Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    view.scene = Some(scene);
    view.update_pictures(paper, waker);
    let primitives = view
        .primitives(TextColors {
            caret: [0.0; 4],
            selection: ui.theme.inactive_selection,
            paper,
        })
        .unwrap();
    let viewport = view.viewport;
    let interface = ui.layers();
    let layers: Vec<_> = interface
        .iter()
        .map(|layer| match layer {
            ui::Layer::Primitives(primitives) => primitives.layer(SCALE),
            ui::Layer::Custom { rect, .. } => draw::Layer {
                scale: viewport.scale,
                origin: [
                    viewport.origin[0] + corner[0] * SCALE,
                    viewport.origin[1] + corner[1] * SCALE,
                ],
                clip: Some(rect.map(|value| value * SCALE)),
                backdrop: Some(ui.theme.paper),
                round: None,
                motion: None,
                primitives: &primitives,
            },
        })
        .collect();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Conflict view"),
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
        label: Some("Conflict view readback"),
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
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |result| result.unwrap());
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(std::time::Duration::from_secs(5)),
        })
        .unwrap();
    let mapped = buffer.get_mapped_range(..).unwrap();
    mapped
        .chunks(row as usize)
        .flat_map(|line| &line[..size[0] as usize * 4])
        .copied()
        .collect()
}

/// A renderer drawing into `output`'s PNGs.
fn gpu(output: &Path) -> (draw::Renderer, wgpu::Device, wgpu::Queue) {
    std::fs::create_dir_all(output).unwrap();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let renderer = draw::Renderer::new(
        device.clone(),
        queue.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );
    (renderer, device, queue)
}

/// Writes RGBA `pixels` of the view as a PNG.
fn save(path: &Path, pixels: &[u8]) {
    let size = SIZE.map(|side| (side * SCALE) as u32);
    let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), size[0], size[1]);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(pixels)
        .unwrap();
}

/// The page shown, a page's versions listed while shown, and the bar's commands: showing
/// opens the newest version, hiding returns to the page.
#[test]
fn conflict_views_offer_the_versions_and_render_offscreen() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("snowbound-conflict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("cache")).unwrap();
    let file = directory.join("synthetic.one");
    std::fs::copy(
        root.join("corpus/conflict-page/candidate/synthetic.one"),
        &file,
    )
    .unwrap();
    let library = Arc::new(crate::Library::section(&file, &directory.join("cache")));
    let section = library.open(&library.location, || {}).unwrap();
    let path = library.location.clone();
    let (mut session, _) = read_session(section, library, path, None).unwrap();
    let page = session.space;
    let version = session.versions(page)[0].space;
    let output = std::env::var_os("SNOWBOUND_CONFLICT_RENDER").map(PathBuf::from);
    let mut gpu = output.as_deref().map(gpu);
    for (name, space, shown, menu, appearance) in [
        ("page", page, None, false, Theme::light()),
        ("versions", page, Some(page), false, Theme::light()),
        ("version", version, Some(page), false, Theme::light()),
        ("version-menu", version, Some(page), true, Theme::light()),
        ("page-dark", page, None, false, Theme::dark()),
        ("version-dark", version, Some(page), false, Theme::dark()),
        (
            "version-menu-dark",
            version,
            Some(page),
            true,
            Theme::dark(),
        ),
    ] {
        session.space = space;
        session.shown = shown;
        let mut ui = Ui::new(appearance, std::time::Duration::from_millis(500));
        // Later frames lay out with earlier frames' measurements, and a menu fades in.
        for _ in 0..8 {
            layout(&mut ui, &session, menu);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        let bar = session.bar();
        match (name, bar) {
            ("page" | "page-dark", Some(Bar::Page { shown: false, .. }))
            | ("versions", Some(Bar::Page { shown: true, .. }))
            | (
                "version" | "version-menu" | "version-dark" | "version-menu-dark",
                Some(Bar::Version { .. }),
            ) => {}
            other => panic!("{other:?}", other = other.0),
        }
        let row = Id::ROOT.child("frame").child("panel").child(version);
        let listed = ui.rect(row).is_some();
        assert_eq!(listed, shown.is_some(), "{name}");
        if let (Some(output), Some((renderer, device, queue))) = (&output, &mut gpu) {
            let shown = session.reader(space)().unwrap();
            let pixels = paint(&ui, shown, renderer, device, queue);
            save(&output.join(format!("conflict-{name}.png")), &pixels);
        }
    }
    // A version reads with its conflicting paragraphs banded as OneNote shows them.
    let kept = session.reader(version)().unwrap();
    let marked = session.versions(page)[0].objects.clone();
    assert!(!marked.is_empty());
    let highlighted = kept.objects.iter().any(|object| match object {
        onestore::page::PageObject::Outline(outline) => {
            outline.paragraphs.iter().any(|paragraph| {
                paragraph
                    .text()
                    .is_some_and(|text| marked.contains(&text.id))
                    && paragraph.format.highlight == Some(canvas::conflict::CONFLICTING)
            })
        }
        _ => false,
    });
    assert!(highlighted);
    session.section.close().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
}

/// A page's versions listed under it while shown, each read-only under OneNote's bar with
/// what it changed banded, from OneNote's own (`corpus/page-versions/native/step-04`).
#[test]
fn page_versions_list_open_read_only_and_render_offscreen() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let directory = std::env::temp_dir().join(format!("snowbound-history-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&directory);
    std::fs::create_dir_all(directory.join("cache")).unwrap();
    let file = directory.join("History.one");
    std::fs::copy(
        root.join("corpus/page-versions/native/step-04/notebook/History.one"),
        &file,
    )
    .unwrap();
    let library = Arc::new(crate::Library::section(&file, &directory.join("cache")));
    let section = library.open(&library.location, || {}).unwrap();
    let path = library.location.clone();
    let (mut session, _) = read_session(section, library, path, None).unwrap();
    let page = session.space;
    let versions: Vec<_> = session
        .page_versions(page)
        .iter()
        .map(|version| version.context)
        .collect();
    assert_eq!(versions.len(), 2);
    assert_eq!(
        crate::history::label(&session.page_versions(page)[1])
            .rsplit_once(' ')
            .map(|(_, who)| who),
        Some("virtual")
    );
    let output = std::env::var_os("SNOWBOUND_CONFLICT_RENDER").map(PathBuf::from);
    let mut gpu = output.as_deref().map(gpu);
    for (name, shown, version, menu, appearance) in [
        ("history-page", None, None, false, Theme::light()),
        ("history-shown", Some(page), None, false, Theme::light()),
        (
            "history-version",
            Some(page),
            Some(versions[0]),
            false,
            Theme::light(),
        ),
        (
            "history-menu",
            Some(page),
            Some(versions[0]),
            true,
            Theme::light(),
        ),
        (
            "history-version-dark",
            Some(page),
            Some(versions[0]),
            false,
            Theme::dark(),
        ),
        (
            "history-menu-dark",
            Some(page),
            Some(versions[0]),
            true,
            Theme::dark(),
        ),
    ] {
        session.shown_history = shown;
        session.version = version;
        assert_eq!(session.read_only(), version.is_some(), "{name}");
        match (session.bar(), version) {
            (
                Some(Bar::History {
                    version: open,
                    grouped: false,
                    ..
                }),
                Some(version),
            ) => {
                assert_eq!(open, version)
            }
            (None, None) => {}
            _ => panic!("{name}"),
        }
        let mut ui = Ui::new(appearance, std::time::Duration::from_millis(500));
        for _ in 0..8 {
            layout(&mut ui, &session, menu);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        for context in &versions {
            let row = Id::ROOT.child("frame").child("panel").child(context);
            assert_eq!(ui.rect(row).is_some(), shown.is_some(), "{name}");
        }
        if let (Some(output), Some((renderer, device, queue))) = (&output, &mut gpu) {
            let shown = match version {
                Some(version) => session.version_reader(page, version)().unwrap(),
                None => session.reader(page)().unwrap(),
            };
            save(
                &output.join(format!("{name}.png")),
                &paint(&ui, shown, renderer, device, queue),
            );
        }
    }
    // The newest version bands the line its author added since the version before it.
    let newest = session.version_reader(page, versions[0])().unwrap();
    let banded: Vec<_> = newest
        .objects
        .iter()
        .flat_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline.paragraphs.clone(),
            _ => Vec::new(),
        })
        .filter(|paragraph| paragraph.format.highlight == Some(canvas::conflict::CHANGED))
        .filter_map(|paragraph| paragraph.text().map(|text| text.text.text().to_owned()))
        .collect();
    assert_eq!(banded, ["Second author line."]);
    session.section.close().unwrap();
    std::fs::remove_dir_all(&directory).unwrap();
}
