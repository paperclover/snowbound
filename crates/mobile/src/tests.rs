use super::*;
use library::{sb_library_set_offline, sb_library_sync_now};
use notebook::session::SyncState;
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
    Section::open(Arc::clone(library), path, "Clover Test".into(), || {}).unwrap()
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
    let library = Arc::new(Library::open(&file, &directory.path().join("cache"), true).unwrap());
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
    let library = Library::open(
        &corpus("m6/native-features-01/notebook"),
        directory.path(),
        true,
    )
    .unwrap();
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
        true,
    )
    .unwrap();
    assert_eq!(lone.tabs().unwrap().len(), 1);
}

#[test]
fn a_new_notebook_opens_with_one_section_in_its_colour_and_refuses_a_taken_name() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("Mine");
    let cache = directory.path().join("cache");
    std::fs::create_dir(&cache).unwrap();
    let c = |path: &Path| CString::new(path.to_str().unwrap()).unwrap();
    let create = |error: &mut *mut c_char| unsafe {
        library::sb_notebook_create(
            c(&root).as_ptr(),
            c(&cache).as_ptr(),
            c"Clover Test".as_ptr(),
            c"Tuesday, September 29, 2026".as_ptr(),
            c"9:41 AM".as_ptr(),
            error,
        )
    };
    let mut error = std::ptr::null_mut();
    assert!(create(&mut error));
    assert!(error.is_null());
    let library = Library::open(&root, &cache, true).unwrap();
    let tabs = library.tabs().unwrap();
    assert_eq!(tabs.len(), 1);
    assert!(tabs[0].readable);
    assert_eq!(library::sb_library_color(&library), 0x91baae);
    assert!(!create(&mut error));
    assert!(!error.is_null());
    unsafe { sb_string_free(error) };
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
    // UIKit's keystrokes, one call each, with the selection set again where it is: one run.
    for text in ["h", "i", "x"] {
        canvas.insert(text.into()).unwrap();
        let selection = canvas.selection().unwrap();
        canvas.select(selection).unwrap();
    }
    assert!(
        canvas
            .page
            .key(&Key::Named(NamedKey::Backspace), None)
            .unwrap()
            .changed
    );
    canvas.insert("!".into()).unwrap();
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
    let library = Arc::new(Library::open(&file, &cache, true).unwrap());
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
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
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
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
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
    let library = Library::open(
        &corpus("m6/native-features-01/notebook"),
        directory.path(),
        true,
    )
    .unwrap();
    let found = library::search(&[&library], None, "collapsed PARENT").unwrap();
    assert!(!found.is_empty());
    assert!(found[0].snippet.to_lowercase().contains("collapsed parent"));
    let [start, end] = found[0].snippet_hits[0];
    let units: Vec<u16> = found[0].snippet.encode_utf16().collect();
    assert_eq!(
        String::from_utf16(&units[start..end])
            .unwrap()
            .to_lowercase(),
        "collapsed"
    );
    assert!(
        library::search(&[&library], None, "no such words anywhere")
            .unwrap()
            .is_empty()
    );
    let section = &found[0].section;
    assert!(
        library::search(&[&library], Some(section), "collapsed parent")
            .unwrap()
            .iter()
            .all(|found| found.section == *section)
    );
}

#[test]
fn search_lists_title_matches_of_every_notebook_first() {
    let directory = tempfile::tempdir().unwrap();
    let personal = Library::open(
        &corpus("search/notebook"),
        &directory.path().join("a"),
        true,
    )
    .unwrap();
    let other = Library::open(
        &corpus("m6/native-features-01/notebook"),
        &directory.path().join("b"),
        true,
    )
    .unwrap();
    let found = library::search(&[&other, &personal], None, "tom").unwrap();
    assert_eq!(found[0].title, "Tomatoes");
    assert_eq!(found[0].notebook, 1);
    assert_eq!(found[0].title_hits, [[0, 3]]);
    assert!(found[1..].iter().all(|found| !found.in_title));
    assert_eq!(found.len(), 5);
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
fn a_picture_goes_in_at_the_caret_and_undoes() {
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
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
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

/// Against a disposable share `agent` (`ONESTORE_SMB_LAB=HOST:PORT`, as
/// `SNOWBOUND_TEST_SMB_USER` with `SNOWBOUND_TEST_SMB_PASSWORD`, or a guest): a notebook
/// folder copied to the share lists, a section opens and publishes typing, and the listing
/// kept in the cache opens the section again while the server cannot be reached.
#[test]
#[ignore = "requires ONESTORE_SMB_LAB pointing to a disposable share"]
fn a_notebook_on_a_share_saves_and_opens_offline() {
    let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
    let variable = |name| std::env::var(name).unwrap_or_default();
    let server = |address: &str| library::Server {
        address: address.into(),
        share: "agent".into(),
        user: variable("SNOWBOUND_TEST_SMB_USER"),
        password: variable("SNOWBOUND_TEST_SMB_PASSWORD"),
        domain: String::new(),
    };
    let client = server(&address).connect().unwrap();
    let root = format!("snowbound-test-mobile-{}", std::process::id());
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
    notebook::location::moved(
        cache.path(),
        &notebook::location::smb(&address, "agent", &root),
        &notebook::location::smb(&format!("{host}:9"), "agent", &root),
    )
    .unwrap();
    let offline = Arc::new(Library::server(unreachable, &root, cache.path()).unwrap());
    assert_eq!(offline.tabs().unwrap()[0].path, "Features.one");
    let reopened = open(&offline, "Features.one");
    let page = reopened
        .shared
        .page(space(&reopened, "Paragraph controls"))
        .unwrap()
        .0;
    assert!(canvas::search::page_text(&page).contains("Shared "));
    client.delete(&format!("{root}/Features.one")).unwrap();
    client.delete(&root).unwrap();
}

#[test]
fn a_share_notebook_syncs_only_its_readable_sections() {
    let (directory, root) = copy("native-encrypted/cold-encrypted-02/notebook");
    std::fs::copy(
        corpus("m6/native-features-01/notebook/Empty.one"),
        root.join("Empty.one"),
    )
    .unwrap();
    let notebook =
        notebook::session::Notebook::open(&root, directory.path().join("cache")).unwrap();
    let mut files = std::collections::BTreeMap::new();
    library::files(notebook.catalog(), &mut files);
    assert_eq!(files.keys().collect::<Vec<_>>(), ["Empty.one"]);
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
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
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
    let after = canvas.page.editor.date().unwrap().timestamp();
    assert_ne!(after, before);
    // `sb_view_date_request` hands the host the date in its seconds.
    assert_eq!(library::unix(after), 1_767_349_800);
}

#[test]
fn pages_open_with_the_snap_default_font_and_markdown_options_set() {
    // The defaults, so pages other tests open meanwhile see no change.
    sb_set_snap_to_grid(true);
    unsafe { sb_set_default_font(c"Calibri".as_ptr(), 11.0) };
    sb_set_markdown_shortcuts(true);
    assert_eq!(
        *options(),
        (true, canvas::editor::DefaultFont::default(), true)
    );
    let (_directory, section) = features();
    let canvas = canvas(&section, "Paragraph controls");
    assert!(canvas.page.snap_to_grid);
    assert_eq!(canvas.page.editor.default_font.face, "Calibri");
    assert!(canvas.page.editor.markdown.is_some());
}

#[test]
fn a_share_notebook_names_its_files_from_the_share_s_top() {
    assert_eq!(library::on_share("", "Notes.one"), "Notes.one");
    assert_eq!(
        library::on_share("Team/Book", "Group/Notes.one"),
        "Team/Book/Group/Notes.one"
    );
}

/// A notebook copy with its library, and a section of it open for editing.
fn notebook_open(path: &str) -> (tempfile::TempDir, PathBuf, Arc<Library>, Section) {
    let (directory, root) = copy("m6/native-features-01/notebook");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
    let section = open(&library, path);
    (directory, root, library, section)
}

/// Waits up to ten seconds for `ready`.
fn eventually(mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn sync_status_lists_every_section_and_work_offline_holds_edits_until_sync_now() {
    let (_directory, root, library, section) = notebook_open("Features.one");
    // Every readable section, the open one's from its session.
    assert!(eventually(|| {
        let status = library.sync_status();
        status.len()
            == library
                .tabs()
                .unwrap()
                .iter()
                .filter(|tab| tab.readable)
                .count()
            && status
                .iter()
                .all(|sync| sync.state == SyncState::UpToDate as u8)
    }));
    sb_library_set_offline(&library, true);
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.insert("Offline ".into()).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    let file = root.join("Features.one");
    let queued = |library: &Library| {
        library
            .sync_status()
            .into_iter()
            .find(|sync| sync.path == "Features.one")
            .unwrap()
            .queued
    };
    assert!(eventually(|| queued(&library) == 1));
    std::thread::sleep(Duration::from_millis(500));
    assert!(!stored_text(&file, canvas.space).contains("Offline "));
    sb_library_sync_now(&library);
    assert!(eventually(|| queued(&library) == 0));
    assert!(stored_text(&file, canvas.space).contains("Offline "));
    sb_library_set_offline(&library, false);
}

#[test]
fn sections_open_the_first_time_while_the_background_syncs() {
    let (directory, root) = copy("m6/native-features-01/notebook");
    let library = Arc::new(Library::open(&root, &directory.path().join("cache"), true).unwrap());
    let rounds = {
        let library = Arc::clone(&library);
        std::thread::spawn(move || {
            for _ in 0..200 {
                sb_library_sync_now(&library);
                std::thread::sleep(Duration::from_millis(2));
            }
        })
    };
    for tab in library
        .tabs()
        .unwrap()
        .into_iter()
        .filter(|tab| tab.readable)
    {
        open(&library, &tab.path);
    }
    rounds.join().unwrap();
    for tab in library
        .tabs()
        .unwrap()
        .into_iter()
        .filter(|tab| tab.readable)
    {
        open(&library, &tab.path);
    }
}

#[test]
fn the_tags_summary_lists_tagged_paragraphs_and_opens_on_them() {
    let (directory, file) = copy("structural-probe/tag-gallery.one");
    let library = Arc::new(Library::open(&file, &directory.path().join("cache"), true).unwrap());
    let path = file.to_string_lossy().into_owned();
    let section = open(&library, &path);
    let tagged = library.tagged().unwrap();
    assert!(tagged.len() >= 9, "{}", tagged.len());
    assert!(
        tagged
            .iter()
            .any(|tag| tag.name == "To Do" && tag.shape == 3)
    );
    let first = &tagged[0];
    let space = first.page.parse().unwrap();
    let (page, _) = section.shared.page(space).unwrap();
    let mut canvas = Canvas::new(space, page, [1206, 2622], 3.0).unwrap();
    assert!(
        canvas
            .select_paragraph(first.paragraph.parse().unwrap())
            .unwrap()
    );
    let [start, end] = canvas.selection().unwrap();
    assert!(canvas.text([start, end]).contains(first.text.trim()));
}

#[test]
fn every_tag_applies_and_reports_and_remove_tag_clears_them() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    for index in [0, 12, 28] {
        assert!(canvas.format(16 + index).unwrap());
        assert_ne!(canvas.format_bits().unwrap() & 1 << (16 + index), 0);
    }
    assert!(canvas.format(8).unwrap());
    assert_eq!(canvas.format_bits().unwrap() >> 16, 0);
    assert!(canvas.format(16 + NoteTag::defaults().len() as u8).is_err());
}

#[test]
fn tag_icons_draw_their_art_and_highlighting_tags_have_none() {
    let mut rgba = vec![0; 32 * 32 * 4];
    for tag in NoteTag::defaults() {
        let drawn = unsafe { sb_tag_icon(tag.shape, true, 32, rgba.as_mut_ptr()) };
        assert_eq!(drawn, tag.shape != 0, "{tag:?}");
        if drawn {
            assert!(rgba.chunks_exact(4).any(|pixel| pixel[3] > 0), "{tag:?}");
        }
    }
}

#[test]
fn page_colour_rule_lines_and_art_are_stored_and_undo() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    assert!(canvas.set_paper(Some(0xfef5ed), Some(1)).unwrap());
    assert!(canvas.set_art(Some("Bamboo")).unwrap());
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    let art = |page: &Page| {
        page.objects
            .iter()
            .filter(|object| {
                matches!(object, onestore::page::PageObject::Image(image) if image.background)
            })
            .count()
    };
    let stored = section.shared.page(canvas.space).unwrap().0;
    assert_eq!(stored.color, Some(0xfef5ed));
    assert_eq!(stored.rule_lines, Some(canvas::template::RULE_LINES[1].1));
    assert_eq!(art(&stored), 1);
    assert!(canvas.set_art(None).unwrap());
    assert!(moved(canvas.page.undo(false).unwrap()));
    assert!(moved(canvas.page.undo(false).unwrap()));
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    let stored = section.shared.page(canvas.space).unwrap().0;
    assert_eq!((stored.color, art(&stored)), (Some(0xfef5ed), 0));
}

#[test]
fn insert_space_turns_the_next_touch_into_a_drag() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    assert_eq!(canvas.target([300.0, 1500.0]), Target::Page);
    let _ = canvas.page.insert_space();
    assert_eq!(canvas.target([300.0, 1500.0]), Target::Grip);
}

/// The page's drawings as the section stores them.
fn stored_ink(section: &Section, space: ExGuid) -> Vec<onestore::page::Ink> {
    let (page, _) = section.shared.page(space).unwrap();
    page.objects
        .into_iter()
        .filter_map(|object| match object {
            onestore::page::PageObject::Ink(ink) => Some(ink),
            _ => None,
        })
        .collect()
}

/// A press, drags through `points` and a release, and the edit it made stored.
fn trace(canvas: &mut Canvas, section: &Section, points: &[[f32; 2]]) {
    canvas.press(points[0]).unwrap();
    for &point in &points[1..] {
        canvas.drag(point).unwrap();
    }
    canvas.release().unwrap();
    if let Some(edit) = canvas.edit().unwrap() {
        section.shared.apply(edit).unwrap();
    }
}

fn line(from: [f32; 2], to: [f32; 2]) -> Vec<[f32; 2]> {
    (0..=20)
        .map(|step| {
            let t = step as f32 / 20.0;
            [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ]
        })
        .collect()
}

/// The tab colour of a section that stores none.
const SECTION: u32 = 0x00e4_a88a;

#[test]
fn pencil_tools_draw_pick_move_delete_and_erase_drawings_as_edits() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    let space = canvas.space;
    let before = stored_ink(&section, space).len();
    // The red pen, second in OneNote's gallery after the section's accent.
    canvas.set_tool(1, 2, SECTION).unwrap();
    trace(&mut canvas, &section, &line([100.0, 600.0], [300.0, 660.0]));
    let drawn = stored_ink(&section, space);
    assert_eq!(drawn.len(), before + 1);
    let stroke = &drawn.last().unwrap().strokes[0];
    assert_eq!(stroke.color, Some(0x241ced));
    assert!(stroke.points.len() > 2);
    // A highlighter stroke is a drawing of its own.
    canvas.set_tool(1, 6, SECTION).unwrap();
    trace(&mut canvas, &section, &line([100.0, 760.0], [300.0, 760.0]));
    assert_eq!(stored_ink(&section, space).len(), before + 2);
    assert!(stored_ink(&section, space).last().unwrap().strokes[0].raster_operation == Some(9));

    // The lasso picks the pen stroke; a touch on it then drags it.
    canvas.set_tool(3, 0, SECTION).unwrap();
    let lasso = [
        [80.0, 580.0],
        [320.0, 580.0],
        [320.0, 700.0],
        [80.0, 700.0],
        [80.0, 582.0],
    ];
    trace(&mut canvas, &section, &lasso);
    let frame = canvas.ink_selection().unwrap();
    assert!(frame[2] > 150.0 && frame[3] > 40.0, "{frame:?}");
    let on = [200.0, 630.0];
    assert_eq!(canvas.target(on), Target::Grip);
    trace(&mut canvas, &section, &[on, [230.0, 645.0], [260.0, 660.0]]);
    let moved = stored_ink(&section, space);
    let layout = &moved[before].layout;
    assert!(layout.x.is_some_and(|x| x > 0.0), "{layout:?}");
    let _ = canvas
        .page
        .key(&Key::Named(NamedKey::Backspace), None)
        .unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert_eq!(stored_ink(&section, space).len(), before + 1);
    assert!(canvas.ink_selection().is_none());

    // The eraser takes the highlighter's stroke it crosses; undo brings it back.
    canvas.set_tool(2, 0, SECTION).unwrap();
    trace(&mut canvas, &section, &line([200.0, 720.0], [200.0, 800.0]));
    assert_eq!(stored_ink(&section, space).len(), before);
    let _ = canvas.page.undo(false).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert_eq!(stored_ink(&section, space).len(), before + 1);
}

#[test]
fn the_accent_pen_and_shapes_draw_as_the_desktop_and_a_cancelled_sweep_stores_nothing() {
    let (_directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    let space = canvas.space;
    let before = stored_ink(&section, space).len();
    let accent = pens(canvas::gpu::colorref(SECTION))[0];
    canvas.set_tool(1, 0, SECTION).unwrap();
    trace(&mut canvas, &section, &line([100.0, 600.0], [300.0, 600.0]));
    let drawn = stored_ink(&section, space);
    assert_eq!(drawn.last().unwrap().strokes[0].color, accent.color);
    assert!(accent.color.is_some());
    // An oval dragged out is one drawing that keeps its shape.
    canvas.set_tool(4, 3, SECTION).unwrap();
    trace(&mut canvas, &section, &line([100.0, 700.0], [300.0, 800.0]));
    let drawn = stored_ink(&section, space);
    assert_eq!(drawn.len(), before + 2);
    assert!(drawn.last().unwrap().shape.is_some());

    // An eraser sweep a second finger cancels gives back what it took and stores nothing.
    canvas.set_tool(2, 0, SECTION).unwrap();
    canvas.press([200.0, 580.0]).unwrap();
    canvas.drag([200.0, 620.0]).unwrap();
    assert!(canvas.page.editor.ink_extent(&[drawn[before].id]).is_none());
    let _ = canvas.page.cancel_ink().unwrap();
    assert!(canvas.edit().unwrap().is_none());
    assert!(canvas.page.editor.ink_extent(&[drawn[before].id]).is_some());
    assert!(!canvas.page.editor.can_redo());
    assert_eq!(stored_ink(&section, space).len(), before + 2);
}

/// Queues an offline edit to Features.one and closes its session, leaving the edit to the
/// background; the replicas in the cache, and the text edited in.
fn unpublished_edit() -> (tempfile::TempDir, PathBuf, Arc<Library>, ExGuid) {
    let (directory, root, library, section) = notebook_open("Features.one");
    sb_library_set_offline(&library, true);
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.insert("Unpublished ".into()).unwrap();
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();
    assert!(eventually(|| !section
        .shared
        .section
        .pending()
        .unwrap()
        .is_empty()));
    let space = canvas.space;
    drop(canvas);
    drop(section);
    (directory, root, library, space)
}

fn replicas(directory: &tempfile::TempDir) -> usize {
    let Ok(folders) = std::fs::read_dir(directory.path().join("cache/replicas")) else {
        return 0;
    };
    folders
        .filter_map(|folder| std::fs::read_dir(folder.ok()?.path()).ok())
        .flatten()
        .filter(|entry| {
            entry
                .as_ref()
                .is_ok_and(|entry| entry.path().extension() == Some("sqlite".as_ref()))
        })
        .count()
}

#[test]
fn closing_publishes_a_pending_edit_so_the_notebook_renames_without_orphans() {
    let (directory, root, library, space) = unpublished_edit();
    library.close(Duration::from_secs(20)).unwrap();
    assert!(stored_text(&root.join("Features.one"), space).contains("Unpublished "));
    drop(library);
    assert!(eventually(|| replicas(&directory) == 0));
    let renamed = directory.path().join("Renamed");
    std::fs::rename(&root, &renamed).unwrap();
    let library = Arc::new(Library::open(&renamed, &directory.path().join("cache"), true).unwrap());
    let section = open(&library, "Features.one");
    let canvas = canvas(&section, "Paragraph controls");
    assert!(stored_text(&renamed.join("Features.one"), canvas.space).contains("Unpublished "));
}

#[test]
fn closing_refuses_while_an_edit_cannot_publish_then_deletes_nothing_pending() {
    let (directory, root, library, space) = unpublished_edit();
    let away = directory.path().join("Away");
    std::fs::rename(&root, &away).unwrap();
    assert!(library.close(Duration::from_secs(2)).is_err());
    assert!(replicas(&directory) > 0);
    std::fs::rename(&away, &root).unwrap();
    library.close(Duration::from_secs(20)).unwrap();
    assert!(stored_text(&root.join("Features.one"), space).contains("Unpublished "));
    drop(library);
    std::fs::remove_dir_all(&root).unwrap();
    assert!(eventually(|| replicas(&directory) == 0));
}

/// Print and Export as PDF make a PDF of the open page on the paper asked for.
#[test]
fn the_page_prints_as_a_pdf() {
    let (_directory, section) = features();
    let title = section.shared.section.pages().unwrap()[0].1.clone();
    let pdf = canvas(&section, &title)
        .pdf(canvas::print::A4, "Features")
        .unwrap();
    assert!(pdf.starts_with(b"%PDF"));
    let text = String::from_utf8_lossy(&pdf);
    assert!(
        text.contains("/MediaBox [0 0 595.276 841.89]"),
        "{}",
        &text[..400]
    );
}

#[test]
fn a_recording_goes_in_as_the_desktop_stores_it_and_a_tap_plays_it() {
    use onestore::page::{PageObject, ParagraphContent};
    let (directory, section) = features();
    let mut canvas = canvas(&section, "Paragraph controls");
    focus(&mut canvas, "Collapsed parent");
    canvas.edit().unwrap();
    let date = ["Tuesday, September 29, 2026", "5:18 PM"];
    assert!(canvas.start_recording(false, date[0], date[1]).unwrap());
    canvas.insert("Linked note".into()).unwrap();
    let samples: Vec<i16> = (0..16_000).map(|n| (n % 64 * 256 - 8192) as i16).collect();
    let wav = canvas::recording::wave(canvas::recording::RATE, &samples);
    assert!(canvas.finish_recording(Some(wav), false).unwrap());
    section
        .shared
        .apply(canvas.edit().unwrap().unwrap())
        .unwrap();

    let page = section.shared.page(canvas.space).unwrap().0;
    let paragraphs: Vec<_> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(&outline.paragraphs),
            _ => None,
        })
        .flatten()
        .collect();
    let file = paragraphs
        .iter()
        .find_map(|paragraph| match &paragraph.content {
            ParagraphContent::Attachment(file) => Some(file),
            _ => None,
        })
        .unwrap();
    let recording = file.recording.unwrap();
    assert_eq!(file.filename, format!("{}.wav", page.title));
    assert_eq!((recording.kind, recording.duration_ms), (1, Some(1000)));
    // IMA ADPCM, as the desktop stores audio for OneNote (`corpus/recording`).
    let bytes = file.bytes.as_deref().unwrap();
    assert_eq!(u16::from_le_bytes([bytes[20], bytes[21]]), 0x11);
    let linked = |text: &str| {
        paragraphs
            .iter()
            .find(|paragraph| {
                paragraph
                    .text()
                    .is_some_and(|t| t.text.text().starts_with(text))
            })
            .map(|paragraph| (paragraph.media.recordings.clone(), paragraph.media.time_ms))
            .unwrap()
    };
    let line = linked("Audio recording started: 5:18 PM Tuesday, September 29, 2026");
    assert_eq!(line, (vec![recording.id], Some(0)));
    assert_eq!(linked("Linked note").0, [recording.id]);

    // A tap on the recording plays it as PCM from the start.
    let tapped = (0..800)
        .flat_map(|y| (0..300).map(move |x| [x as f32 * 2.0, y as f32 * 2.0]))
        .find(|&point| {
            matches!(
                canvas.page.hit(canvas.device(point.map(|v| v * POINT))),
                Some(Hit::File(_))
            )
        })
        .unwrap()
        .map(|v| v * POINT);
    canvas.press(tapped).unwrap();
    canvas.release().unwrap();
    let folder = directory.path().join("playback");
    let request = canvas.take_play(&folder).unwrap().unwrap();
    assert_eq!(request["at_ms"], 0);
    assert!(request["video"].is_null());
    let sound = std::fs::read(request["sound"].as_str().unwrap()).unwrap();
    assert_eq!(u16::from_le_bytes([sound[20], sound[21]]), 1);
    assert!(canvas.take_play(&folder).unwrap().is_none());
    canvas.played(Some(5_000)).unwrap();
    canvas.played(None).unwrap();
}

#[test]
fn a_movie_from_the_camera_becomes_the_desktop_s_avi() {
    let movie = recording::sb_movie_new();
    let bgra: Vec<u8> = (0..480 * 640 * 4).map(|n| (n % 251) as u8).collect();
    for index in 0..20u64 {
        unsafe {
            recording::sb_movie_picture(
                &mut *movie,
                index * 50_000,
                bgra.as_ptr(),
                640,
                480,
                640 * 4,
            )
        };
    }
    let pcm = vec![0u8; canvas::recording::RATE as usize * 2];
    unsafe { recording::sb_movie_sound(&mut *movie, pcm.as_ptr(), pcm.len()) };
    let mut length = 0;
    let avi = unsafe { recording::sb_movie_finish(movie, 1_000_000, &mut length) };
    let bytes = unsafe { std::slice::from_raw_parts(avi, length) }.to_vec();
    unsafe { sb_bytes_free(avi, length) };
    let parsed = canvas::recording::video::Movie::parse(&bytes).unwrap();
    assert_eq!((parsed.frames.len(), parsed.duration_ms()), (15, 1000));
    assert!(parsed.sound.is_some());
}

fn levels(section: &Section) -> Vec<(ExGuid, u32)> {
    let pages = section.shared.section.pages().unwrap();
    pages
        .into_iter()
        .map(|(space, _, level)| (space, level))
        .collect()
}

#[test]
fn pages_drag_indent_and_undo_as_one_edit_each() {
    let (_directory, _root, _library, section) = notebook_open("Features.one");
    let before = levels(&section);
    assert!(before.len() > 2);
    let last = before.last().unwrap().0;
    let mut dragged = before.clone();
    let moved = dragged.pop().unwrap();
    dragged.insert(0, moved);
    let revisions = section.shared.section.pending().unwrap().len();
    assert!(section.arrange(&dragged, &[last]).unwrap());
    assert_eq!(levels(&section), dragged);
    assert_eq!(
        section.shared.section.pending().unwrap().len(),
        revisions + 1
    );
    let second = dragged[1].0;
    let mut indented = dragged.clone();
    indented[1].1 = 2;
    assert!(section.arrange(&indented, &[second]).unwrap());
    assert_eq!(levels(&section), indented);
    assert!(section.arrange(&dragged, &[second]).unwrap());
    assert!(section.arrange(&before, &[last]).unwrap());
    assert_eq!(levels(&section), before);
    assert!(!section.arrange(&before, &[ExGuid::default()]).unwrap());
}

#[test]
fn a_page_moves_to_another_section_and_back_where_it_was() {
    let (_directory, root, library, features) = notebook_open("Features.one");
    let before = levels(&features);
    let (space, level) = before[1];
    let title = features.shared.section.page(space).unwrap().title;
    let date = ["Thursday, October 1, 2026", "9:00 AM"];
    let moved = library
        .move_page(
            "Features.one",
            space,
            "Empty.one",
            None,
            1,
            None,
            "Clover Test",
            date,
        )
        .unwrap();
    let arrived: ExGuid = moved.page.parse().unwrap();
    assert_eq!(moved.added, None);
    assert_eq!(moved.before, Some(before[2].0.to_string()));
    assert_eq!(moved.level, level);
    assert!(levels(&features).iter().all(|(page, _)| *page != space));
    let empty = open(&library, "Empty.one");
    let listed = empty.shared.section.pages().unwrap();
    assert_eq!(listed.last().unwrap(), &(arrived, title.clone(), 1));
    drop(empty);
    let back = library
        .move_page(
            "Empty.one",
            arrived,
            "Features.one",
            moved.before.map(|before| before.parse().unwrap()),
            moved.level,
            None,
            "Clover Test",
            date,
        )
        .unwrap()
        .page
        .parse()
        .unwrap();
    let mut returned = before.clone();
    returned[1].0 = back;
    assert_eq!(levels(&features), returned);
    assert_eq!(features.shared.section.page(back).unwrap().title, title);
    drop(features);
    library.close(Duration::from_secs(20)).unwrap();
    let arena = onestore::Arena::default();
    let file = onestore::read_file(root.join("Features.one")).unwrap();
    let stored = onestore::Section::open(&arena, file).unwrap();
    assert_eq!(stored.page(back).unwrap().title, title);
}

#[test]
fn a_sections_last_page_moving_leaves_a_page_that_undo_takes_out() {
    let (_directory, _root, library, features) = notebook_open("Features.one");
    let date = ["Thursday, October 1, 2026", "9:02 AM"];
    let page = features.new_page(None, date[0], date[1]).unwrap();
    let arrived: ExGuid = library
        .move_page(
            "Features.one",
            page,
            "Empty.one",
            None,
            1,
            None,
            "Clover Test",
            date,
        )
        .unwrap()
        .page
        .parse()
        .unwrap();
    assert_eq!(levels(&open(&library, "Empty.one")), [(arrived, 1)]);
    let returned = library
        .move_page(
            "Empty.one",
            arrived,
            "Features.one",
            None,
            1,
            None,
            "Clover Test",
            date,
        )
        .unwrap();
    let back = returned.page.parse().unwrap();
    let added: ExGuid = returned.added.unwrap().parse().unwrap();
    assert_eq!(levels(&open(&library, "Empty.one")), [(added, 1)]);
    library
        .move_page(
            "Features.one",
            back,
            "Empty.one",
            None,
            1,
            Some(added),
            "Clover Test",
            date,
        )
        .unwrap();
    assert_eq!(levels(&open(&library, "Empty.one")).len(), 1);
}

#[test]
fn sections_reorder_and_move_between_groups_and_back() {
    let (_directory, root, library, features) = notebook_open("Features.one");
    drop(features);
    let top = |library: &Library| -> Vec<String> {
        let tabs = library.tabs().unwrap();
        tabs.into_iter()
            .filter(|tab| tab.group.is_empty())
            .map(|tab| tab.path)
            .collect()
    };
    let before = top(&library);
    let mut reversed = before.clone();
    reversed.reverse();
    assert_eq!(
        library.place("Empty.one", "", &reversed).unwrap(),
        "Empty.one"
    );
    assert_eq!(top(&library), reversed);
    assert_eq!(
        library.place("Empty.one", "Group A", &[]).unwrap(),
        "Group A/Empty.one"
    );
    assert!(root.join("Group A/Empty.one").exists());
    let tabs = library.tabs().unwrap();
    let grouped: Vec<&str> = tabs
        .iter()
        .filter(|tab| tab.group == "Group A")
        .map(|tab| tab.path.as_str())
        .collect();
    assert_eq!(grouped.last(), Some(&"Group A/Empty.one"));
    assert_eq!(
        library.place("Group A/Empty.one", "", &before).unwrap(),
        "Empty.one"
    );
    assert_eq!(top(&library), before);
    assert!(root.join("Empty.one").exists());
    open(&library, "Empty.one");
}
