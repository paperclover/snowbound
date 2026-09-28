use super::*;
use std::path::{Path, PathBuf};

const CORPUS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus");

fn corpus(path: &str) -> PathBuf {
    Path::new(CORPUS).join(path)
}

/// A copy of the corpus section or notebook folder at `path` in a fresh directory, with a
/// cache beside it.
fn copy(path: &str) -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let source = corpus(path);
    let target = directory.path().join(source.file_name().unwrap());
    let status = std::process::Command::new("cp")
        .arg("-R")
        .arg(&source)
        .arg(&target)
        .status()
        .unwrap();
    assert!(status.success());
    (directory, target)
}

fn open(library: &Arc<Library>, path: &str) -> Section {
    let section = library.section(path, || {}).unwrap();
    Section {
        shared: Arc::new(Shared {
            section,
            author: "Clover Test".into(),
            status: Default::default(),
        }),
        library: Arc::clone(library),
    }
}

fn space(section: &Section, title: &str) -> ExGuid {
    section
        .shared
        .section
        .pages()
        .unwrap()
        .into_iter()
        .find(|(_, heading, _)| heading == title)
        .unwrap()
        .0
}

/// A page of the Features section on a 402 × 874 point phone at 3 pixels per point, at
/// 100% with the page origin at the view's corner.
fn canvas(section: &Section, title: &str) -> Canvas {
    let space = space(section, title);
    let (page, _) = section.shared.page(space).unwrap();
    let mut canvas = Canvas::new(space, page, [1206, 2622], 3.0).unwrap();
    canvas.set_transform(1.0, [0.0; 2]);
    canvas
}

fn features() -> (tempfile::TempDir, Section) {
    let (directory, file) = copy("media-edit/candidate/Features.one");
    let library = Arc::new(Library::open(&file, &directory.path().join("cache")).unwrap());
    let section = open(&library, &file.to_string_lossy());
    (directory, section)
}

/// A view point on the outline holding `text`, `fraction` of the way along its first line.
fn point_on(canvas: &Canvas, text: &str, fraction: f32) -> [f32; 2] {
    let outline = canvas
        .page
        .editor
        .visible_outlines()
        .find(|outline| outline.shown_text().contains(text))
        .unwrap();
    let bounds = outline.bounds();
    let x = bounds.x0 as f32 + (bounds.x1 - bounds.x0) as f32 * fraction;
    let y = bounds.y0 as f32 + 6.0;
    [x * POINT, y * POINT]
}

/// Focuses the outline holding `text` with a tap, and hands the section the edit that took.
fn focus(canvas: &mut Canvas, text: &str) {
    let point = point_on(canvas, text, 0.1);
    canvas.press(point).unwrap();
    canvas.release().unwrap();
}

#[test]
fn a_notebook_folder_lists_sections_in_order_with_groups_and_colours() {
    let directory = tempfile::tempdir().unwrap();
    let library =
        Library::open(&corpus("m6/native-features-01/notebook"), directory.path()).unwrap();
    let tabs = library.tabs().unwrap();
    let names: Vec<_> = tabs
        .iter()
        .map(|tab| (tab.group.as_str(), tab.name.as_str()))
        .collect();
    assert!(names.contains(&("", "Features")), "{names:?}");
    assert!(tabs.iter().any(|tab| !tab.group.is_empty()));
    let lone = Library::open(
        &corpus("media-edit/candidate/Features.one"),
        directory.path(),
    )
    .unwrap();
    assert_eq!(lone.tabs().unwrap().len(), 1);
}

#[test]
fn touches_route_to_text_the_page_and_the_focused_outlines_grip() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    let text = point_on(&canvas, "Collapsed parent", 0.1);
    let first = canvas.target(text);
    assert!(
        matches!(first, Target::ActiveText | Target::Text),
        "{first:?}"
    );
    assert!(canvas.press(text).unwrap());
    canvas.release().unwrap();
    assert_eq!(canvas.target(text), Target::ActiveText);
    // The grip strip sits above the text; a finger reaches it from 16 points higher still.
    let bounds = canvas.active().bounds();
    let above = [
        (bounds.x0 as f32 + 40.0) * POINT,
        (bounds.y0 as f32 - 12.0) * POINT - 10.0,
    ];
    assert_eq!(canvas.target(above), Target::Grip);
    // Unfocused, the grips hide and a touch there reaches the page.
    let _ = canvas.page.focus_changed(false).unwrap();
    assert_eq!(canvas.target(above), Target::Page);
}

#[test]
fn dragging_the_grip_moves_the_outline_onto_the_grid_as_an_edit() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.edit().unwrap();
    let before = canvas.active().origin();
    let grip = [(before[0] + 40.0) * POINT, (before[1] - 8.0) * POINT];
    assert_eq!(canvas.target(grip), Target::Grip);
    canvas.press(grip).unwrap();
    for step in 1..=10 {
        let step = step as f32;
        canvas
            .drag([grip[0] + step * 10.0, grip[1] + step * 7.0])
            .unwrap();
    }
    canvas.release().unwrap();
    let after = canvas.active().origin();
    let margin = canvas.page.editor.margin_origin();
    assert_ne!(after, before);
    for axis in 0..2 {
        let cells = (after[axis] - margin[axis]) / 18.0;
        assert!(
            (cells - cells.round()).abs() < 1e-3,
            "{after:?} off the grid"
        );
    }
    assert!(canvas.edit().unwrap().is_some());
    assert!(canvas.page.editor.can_undo());
}

#[test]
fn edits_leave_the_viewport_where_the_host_put_it() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    // Scrolled far past the content, where the canvas would clamp and reveal.
    canvas.set_transform(1.0, [-2000.0, 5000.0]);
    let origin = canvas.page.viewport.origin;
    focus(&mut canvas, "Collapsed parent");
    canvas.insert("typed".into()).unwrap();
    assert_eq!(canvas.page.viewport.origin, origin);
}

#[test]
fn the_text_model_skips_collapsed_paragraphs_and_round_trips_selection() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    let shown = canvas.active().shown_text();
    let length = canvas.length();
    assert_eq!(length as usize, shown.encode_utf16().count());
    assert_eq!(canvas.text([0, length]), shown);
    let start = shown.find("Collapsed").unwrap() as u32;
    canvas.select([start, start + 9]).unwrap();
    assert_eq!(canvas.selection().unwrap(), [start, start + 9]);
    assert!(!canvas.range_rects([start, start + 9]).unwrap().is_empty());
    let caret = canvas.caret_rect(start).unwrap();
    let closest = canvas
        .closest([caret[0] + 1.0, caret[1] + caret[3] / 2.0])
        .unwrap();
    assert!(closest.abs_diff(start) <= 1, "{closest} for {start}");
    // Every offset has a caret, including those around the collapsed children.
    for offset in 0..=length {
        canvas.caret_rect(offset).unwrap();
    }
}

#[test]
fn select_more_widens_from_the_caret_paragraph_to_the_outline() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    let shown = canvas.active().shown_text();
    let units = |text: &str| text.encode_utf16().count() as u32;
    let start = units(&shown[..shown.find("Collapsed parent").unwrap()]);
    let line = units(
        shown
            .split('\n')
            .find(|line| line.contains("Collapsed parent"))
            .unwrap(),
    );
    canvas.select([start + 2; 2]).unwrap();
    canvas.page.editor.widen_selection().unwrap();
    let [lo, hi] = canvas.selection().unwrap();
    assert!(
        lo == start && [start + line, start + line + 1].contains(&hi),
        "{lo}..{hi}"
    );
    for _ in 0..8 {
        canvas.page.editor.widen_selection().unwrap();
    }
    assert_eq!(canvas.selection().unwrap(), [0, canvas.length()]);
}

#[test]
fn undo_restores_typing() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    let before = canvas.active().shown_text();
    canvas.insert("hi".into()).unwrap();
    assert_ne!(canvas.active().shown_text(), before);
    assert!(moved(canvas.page.undo(false).unwrap()));
    assert_eq!(canvas.active().shown_text(), before);
    assert!(canvas.page.editor.can_redo());
}

/// The text of the page `space` of the section file `file`, read cold.
fn stored_text(file: &Path, space: ExGuid) -> String {
    let arena = onestore::Arena::default();
    let section = onestore::Section::open(&arena, onestore::read_file(file).unwrap()).unwrap();
    canvas::search::page_text(&section.page(space).unwrap())
}

#[test]
fn typing_is_stored_in_the_cache_and_published_to_the_file() {
    let (directory, file) = copy("media-edit/candidate/Features.one");
    let cache = directory.path().join("cache");
    let library = Arc::new(Library::open(&file, &cache).unwrap());
    let section = open(&library, &file.to_string_lossy());
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    let shown = canvas.active().shown_text();
    let at = shown[..shown.find("Collapsed parent").unwrap()]
        .encode_utf16()
        .count() as u32;
    canvas.select([at, at]).unwrap();
    canvas.insert("Saved ".into()).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert!(section.flush(Duration::from_secs(20)));
    let space = canvas.space;
    assert!(stored_text(&file, space).contains("Saved Collapsed parent"));
    // The replica reopens from the cache as the file now holds it.
    let shared = Arc::clone(&section.shared);
    drop((canvas, section));
    Arc::into_inner(shared).unwrap().section.close().unwrap();
    let reopened = open(&library, &file.to_string_lossy());
    let (page, read_only) = reopened.shared.page(space).unwrap();
    assert!(!read_only);
    assert!(canvas::search::page_text(&page).contains("Saved Collapsed parent"));
}

#[test]
fn a_new_page_and_subpage_list_after_their_parent_and_delete_to_the_recycle_bin() {
    let (directory, root) = copy("m6/native-features-01/notebook");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache")).unwrap());
    let section = open(&library, "Features.one");
    let first = section.rows().unwrap()[0].id.parse().unwrap();
    let page = section
        .new_page(None, "Sunday, September 27, 2026", "3:04 AM")
        .unwrap();
    let subpage = section
        .new_page(Some(first), "Sunday, September 27, 2026", "3:05 AM")
        .unwrap();
    let rows = section.rows().unwrap();
    assert_eq!(rows.last().unwrap().id, page.to_string());
    assert_eq!(rows[1].id, subpage.to_string());
    assert_eq!(rows[1].level, rows[0].level + 1);
    let mut canvas = Canvas::new(
        subpage,
        section.shared.page(subpage).unwrap().0,
        [1206, 2622],
        3.0,
    )
    .unwrap();
    assert!(canvas.focus_title().unwrap());
    canvas.insert("Sketches".into()).unwrap();
    assert_eq!(canvas.title().unwrap().shown_text(), "Sketches");
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert!(
        section
            .rows()
            .unwrap()
            .iter()
            .any(|row| row.title == "Sketches")
    );
    section
        .delete_page(subpage, "Sunday, September 27, 2026", "3:06 AM")
        .unwrap();
    assert!(
        section
            .rows()
            .unwrap()
            .iter()
            .all(|row| row.id != subpage.to_string())
    );
    assert!(
        root.join("OneNote_RecycleBin/OneNote_DeletedPages.one")
            .exists()
    );
}

#[test]
fn conflict_pages_list_under_their_page_and_take_no_edits() {
    let (directory, root) = copy("conflict-page/candidate");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache")).unwrap());
    let section = open(&library, "synthetic.one");
    let rows = section.rows().unwrap();
    let version = rows
        .iter()
        .find_map(|row| row.versions.first())
        .expect("a conflict page");
    assert!(!version.user.is_empty());
    let (page, read_only) = section.shared.page(version.id.parse().unwrap()).unwrap();
    assert!(read_only);
    // Conflicting paragraphs carry OneNote's highlight.
    let highlighted = page.objects.iter().any(|object| match object {
        onestore::page::PageObject::Outline(outline) => outline
            .paragraphs
            .iter()
            .any(|paragraph| paragraph.format.highlight == Some(canvas::conflict::CONFLICTING)),
        _ => false,
    });
    assert!(highlighted);
}

#[test]
fn search_finds_pages_by_title_and_text_across_sections() {
    let directory = tempfile::tempdir().unwrap();
    let library =
        Library::open(&corpus("m6/native-features-01/notebook"), directory.path()).unwrap();
    let found = library.search(None, "collapsed PARENT").unwrap();
    assert!(!found.is_empty());
    assert!(found[0].snippet.to_lowercase().contains("collapsed parent"));
    assert!(
        library
            .search(None, "no such words anywhere")
            .unwrap()
            .is_empty()
    );
}

#[test]
fn find_selects_the_match_and_formatting_reports_and_toggles() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    assert!(canvas.find("collapsed PARENT").unwrap());
    let [start, end] = canvas.selection().unwrap();
    assert_eq!(canvas.text([start, end]).to_lowercase(), "collapsed");
    assert_eq!(canvas.format_bits().unwrap() & 1, 0);
    assert!(canvas.format(0).unwrap());
    assert_eq!(canvas.format_bits().unwrap() & 1, 1);
    // To Do on the paragraph.
    assert!(canvas.format(16).unwrap());
    assert_ne!(canvas.format_bits().unwrap() & 1 << 16, 0);
    assert!(canvas.edit().unwrap().is_some());
}

#[test]
fn a_picture_goes_after_the_caret_paragraph_and_undoes() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.edit().unwrap();
    let png = std::fs::read(corpus(
        "../crates/snowbound/assets/icon/Snowbound-SnowLeopard.png",
    ))
    .unwrap();
    assert!(canvas.insert_picture(&png, [64.0, 64.0]).unwrap());
    let edit = canvas.edit().unwrap().unwrap();
    assert!(edit.ops.iter().any(|op| matches!(
        op,
        Op::Page {
            op: onestore::op::PageOp::Insert { .. },
            ..
        }
    )));
    section.shared.apply(edit).unwrap();
    let pictures = |page: &Page| {
        page.objects
            .iter()
            .filter_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => Some(
                    outline
                        .paragraphs
                        .iter()
                        .filter(|paragraph| {
                            matches!(
                                paragraph.content,
                                onestore::page::ParagraphContent::Image(_)
                            )
                        })
                        .count(),
                ),
                _ => None,
            })
            .sum::<usize>()
    };
    let before = pictures(&section.shared.page(canvas.space).unwrap().0);
    assert!(moved(canvas.page.undo(false).unwrap()));
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert_eq!(
        pictures(&section.shared.page(canvas.space).unwrap().0),
        before - 1
    );
}

#[test]
fn to_do_marks_a_new_pages_body() {
    let (directory, root) = copy("m6/native-features-01/notebook");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache")).unwrap());
    let section = open(&library, "Features.one");
    let page = section
        .new_page(None, "Sunday, September 27, 2026", "3:04 AM")
        .unwrap();
    let mut canvas = Canvas::new(
        page,
        section.shared.page(page).unwrap().0,
        [1206, 2622],
        3.0,
    )
    .unwrap();
    canvas.set_transform(1.0, [0.0; 2]);
    assert!(canvas.focus_title().unwrap());
    canvas.insert("Groceries".into()).unwrap();
    canvas.insert("\n".into()).unwrap();
    canvas.insert("Oat milk".into()).unwrap();
    canvas.insert("\n".into()).unwrap();
    canvas.insert("Pears".into()).unwrap();
    assert!(canvas.format(16).unwrap());
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    let stored = section.shared.page(page).unwrap().0;
    let tags: usize = stored
        .objects
        .iter()
        .map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .filter_map(|paragraph| paragraph.text())
                .map(|text| text.tags.len())
                .sum(),
            _ => 0,
        })
        .sum();
    assert_eq!(tags, 1);
}

/// Against the Samba lab (`ONESTORE_SMB_LAB=127.0.0.1:PORT`, share `agent`, guest): a
/// notebook folder copied to the share lists, a section opens and publishes typing, and the
/// listing kept in the cache opens the section again while the server cannot be reached.
#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to disposable Samba"]
fn a_notebook_on_a_share_saves_and_opens_offline() {
    let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
    let server = |address: &str| library::Server {
        address: address.into(),
        share: "agent".into(),
        user: String::new(),
        password: String::new(),
        domain: String::new(),
    };
    let client = server(&address).connect().unwrap();
    let root = format!("mobile-{}", std::process::id());
    client.create_directory(&root).unwrap();
    let source = corpus("media-edit/candidate/Features.one");
    client
        .create(
            &format!("{root}/Features.one"),
            &std::fs::read(&source).unwrap(),
        )
        .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let library = Arc::new(Library::server(server(&address), &root, cache.path()).unwrap());
    assert_eq!(library.tabs().unwrap()[0].path, "Features.one");
    let section = open(&library, "Features.one");
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.insert("Shared ".into()).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert!(section.flush(Duration::from_secs(30)));
    let published = client
        .read(&format!("{root}/Features.one"), 1 << 26)
        .unwrap();
    let arena = onestore::Arena::default();
    let stored = onestore::Section::open(&arena, published).unwrap();
    assert!(canvas::search::page_text(&stored.page(canvas.space).unwrap()).contains("Shared "));
    let shared = Arc::clone(&section.shared);
    drop((canvas, section, library));
    Arc::into_inner(shared).unwrap().section.close().unwrap();
    // The same notebook while its server does not answer: a closed port on the lab's host.
    let host = address.split(':').next().unwrap();
    let mut unreachable = server(&format!("{host}:9"));
    unreachable.share = "agent".into();
    let listing = |address: &str| {
        cache.path().join("smb").join(format!(
            "{}.json",
            format!("{address}/agent/{root}").replace(|c: char| !c.is_alphanumeric(), "_")
        ))
    };
    std::fs::copy(listing(&address), listing(&format!("{host}:9"))).unwrap();
    let offline = Arc::new(Library::server(unreachable, &root, cache.path()).unwrap());
    assert_eq!(offline.tabs().unwrap()[0].path, "Features.one");
    let reopened = open(&offline, "Features.one");
    let page = reopened
        .shared
        .page(space(&reopened, "Paragraph controls"))
        .unwrap()
        .0;
    assert!(canvas::search::page_text(&page).contains("Shared "));
}

/// Paths the test coordinator was asked for, with whether to write.
static COORDINATED: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

extern "C" fn coordinator(
    path: *const c_char,
    write: bool,
    body: extern "C" fn(*mut c_void),
    context: *mut c_void,
) {
    COORDINATED.lock().unwrap().push((string(path), write));
    body(context);
}

#[test]
fn a_local_sections_reads_and_publications_go_through_the_hosts_coordination() {
    library::sb_set_coordinator(coordinator);
    let (directory, root) = copy("m6/native-features-01/notebook");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache")).unwrap());
    let section = open(&library, "Features.one");
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.insert("Coordinated ".into()).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert!(section.flush(Duration::from_secs(20)));
    let file = root.canonicalize().unwrap().join("Features.one");
    let file = file.to_string_lossy();
    let coordinated = COORDINATED.lock().unwrap();
    assert!(
        coordinated
            .iter()
            .any(|(path, write)| path == &*file && !write)
    );
    assert!(
        coordinated
            .iter()
            .any(|(path, write)| path == &*file && *write)
    );
}

#[test]
fn tapping_the_date_asks_for_it_and_a_new_date_is_stored() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    // The first view point, scanning down the page, that lands on the date.
    let date = (0..400)
        .flat_map(|y| (0..130).map(move |x| [x as f32 * 3.0, y as f32 * 3.0]))
        .find(|point| {
            matches!(
                canvas.page.hit(canvas.device(*point)),
                Some(Hit::Date(DateField::Date))
            )
        })
        .expect("the page shows its date");
    canvas.press(date).unwrap();
    canvas.release().unwrap();
    assert_eq!(canvas.asked.take(), Some(DateField::Date));
    let before = canvas.page.editor.date().unwrap().timestamp();
    // 2 January 2026, 10:30 UTC.
    assert!(
        canvas
            .change_date(
                1_767_349_800,
                ["Friday, January 2, 2026".into(), "10:30 AM".into()]
            )
            .unwrap()
    );
    let edit = canvas.edit().unwrap().unwrap();
    assert!(edit.ops.iter().any(|op| matches!(
        op,
        Op::Page {
            op: PageOp::Date { .. },
            ..
        }
    )));
    section.shared.apply(edit).unwrap();
    let stored = section.shared.page(canvas.space).unwrap().0.date_text();
    assert_eq!(stored.unwrap()[0], "Friday, January 2, 2026");
    assert_ne!(canvas.page.editor.date().unwrap().timestamp(), before);
}
