use super::*;

const FEATURES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../corpus/media-edit/candidate/Features.one"
);

/// A page of the corpus section on a 402 × 874 point phone at 3 pixels per point, at 100%
/// with the page origin at the view's corner.
fn canvas(title: &str) -> Canvas {
    let section = Section::open(&std::fs::read(FEATURES).unwrap()).unwrap();
    let index = section
        .headings
        .iter()
        .position(|(heading, _)| heading.to_str().unwrap() == title)
        .unwrap();
    let mut canvas = Canvas::new(section.pages[index].clone(), [1206, 2622], 3.0).unwrap();
    canvas.set_transform(1.0, [0.0; 2]);
    canvas
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

#[test]
fn the_section_lists_titles_and_levels() {
    let section = Section::open(&std::fs::read(FEATURES).unwrap()).unwrap();
    assert!(
        section
            .headings
            .iter()
            .any(|(title, _)| title.to_str() == Ok("Paragraph controls"))
    );
    assert!(section.headings.iter().all(|(_, level)| *level >= 1));
}

#[test]
fn a_notebook_folder_lists_sections_in_order_with_groups_and_colours() {
    let root = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../corpus/m6/native-features-01/notebook"
    );
    let notebook = listing(Path::new(root)).unwrap();
    let names: Vec<_> = notebook
        .sections
        .iter()
        .map(|tab| (tab.group.as_str(), tab.name.as_str()))
        .collect();
    eprintln!("{names:?}");
    assert!(names.contains(&("", "Features")));
    assert!(notebook.sections.iter().any(|tab| !tab.group.is_empty()));
    let lone = listing(Path::new(FEATURES)).unwrap();
    assert_eq!(lone.sections.len(), 1);
}

#[test]
fn touches_route_to_text_the_page_and_the_focused_outlines_grip() {
    let mut canvas = canvas("Paragraph controls");
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
    let mut canvas = canvas("Paragraph controls");
    let text = point_on(&canvas, "Collapsed parent", 0.1);
    canvas.press(text).unwrap();
    canvas.release().unwrap();
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
fn the_text_model_skips_collapsed_paragraphs_and_round_trips_selection() {
    let mut canvas = canvas("Paragraph controls");
    let text = point_on(&canvas, "Collapsed parent", 0.1);
    canvas.press(text).unwrap();
    canvas.release().unwrap();
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
fn undo_restores_typing() {
    let mut canvas = canvas("Paragraph controls");
    let text = point_on(&canvas, "Collapsed parent", 0.1);
    canvas.press(text).unwrap();
    canvas.release().unwrap();
    let before = canvas.active().shown_text();
    canvas.insert("hi".into()).unwrap();
    assert_ne!(canvas.active().shown_text(), before);
    assert!(moved(canvas.page.undo(false).unwrap()));
    assert_eq!(canvas.active().shown_text(), before);
    assert!(canvas.page.editor.can_redo());
}
