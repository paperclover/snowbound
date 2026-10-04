use super::*;
use crate::document::{TextDocument, TextPosition};
use crate::editor::Toggle;
use crate::gpu::painted_layout;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::collections::BTreeSet;
use std::sync::Arc;

const COLORS: TextColors = TextColors {
    caret: [0.1, 0.3, 0.9, 1.0],
    selection: [0.7, 0.8, 1.0, 1.0],
    paper: crate::gpu::Paper::WHITE,
};

static NO_ART: std::sync::LazyLock<crate::gpu::TagArt> = std::sync::LazyLock::new(Default::default);

/// Paint for a view at `scale` device pixels per point on a display of `display_scale`.
pub(super) fn paint(show_caret: bool, scale: f32, display_scale: f32) -> Paint<'static> {
    Paint {
        caret: f32::from(u8::from(show_caret)),
        scale,
        device_origin: [0.0; 2],
        pixel: display_scale / scale,
        colors: COLORS,
        visible: [f32::NEG_INFINITY, f32::INFINITY],
        chrome: true,
        found: &[],
        played: None,
        spelling: None,
        tag_art: &NO_ART,
    }
}

#[test]
fn date_buttons_keep_accessibility_identity_and_match_mouse_hits_after_reflow() {
    let mut engine = TextEngine::default();
    let mut fields = [vec!["Header"], vec!["Monday", "6:14 AM"]].map(|text| {
        TextOutline::new(
            &mut engine,
            TextDocument::new(
                text.into_iter()
                    .map(|text| Paragraph::new(text.into(), Default::default()))
                    .collect(),
            )
            .unwrap(),
            468.0,
            [0.0; 2],
        )
        .unwrap()
        .snapshot()
    });
    fields[0].title = true;
    let page = Page {
        identity: None,
        title: "Header".into(),
        created: Some(1),
        margin_origin: [36.0, 14.4],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects: vec![onestore::page::PageObject::Title(onestore::page::Title {
            id: onestore::ExGuid::default(),
            date: Some(fields[1].id),
            layout: Default::default(),
            outlines: fields.into(),
        })],
    };
    let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
    let scene = (scene, [20.0, 40.0]);
    let viewport = Viewport {
        size: [1000, 700],
        scale: 2.0,
        origin: [-30.0, -50.0],
    };
    let mut access = accessibility::Accessibility::default();
    let mut tree = None;
    let mut identities = Vec::new();
    for phase in 0..3 {
        if phase == 1 {
            editor
                .insert(&mut engine, &"A wrapped title ".repeat(25))
                .unwrap();
        }
        if phase == 2 {
            editor
                .change_date(&mut engine, 2, ["Wednesday".into(), "8:40 AM".into()])
                .unwrap();
        }
        let update = access
            .update(&editor, Some(&scene), viewport, "Header", None, None)
            .unwrap();
        let buttons: Vec<_> =
            accessibility::tests::nodes(accessibility::tests::apply(&mut tree, update))
                .into_iter()
                .filter(|node| node.role() == accesskit::Role::Button)
                .collect();
        assert_eq!(buttons.len(), 2);
        for (index, node) in buttons.iter().enumerate() {
            let id = node.locate().0;
            let field = [DateField::Date, DateField::Time][index];
            assert_eq!(access.date_for_node(id), Some(field));
            assert!(node.data().supports_action(accesskit::Action::Click));
            let rect = node.bounding_box().unwrap();
            let point = viewport.document_point([
                ((rect.x0 + rect.x1) * 0.5) as f32,
                ((rect.y0 + rect.y1) * 0.5) as f32,
            ]);
            assert_eq!(
                page_hit_test(&editor, Some(&scene), point, 0.5),
                Some(Hit::Date(field))
            );
            if phase == 0 {
                identities.push(id);
            } else {
                assert_eq!(id, identities[index]);
            }
        }
        if phase == 2 {
            assert_eq!(buttons[1].data().value(), Some("8:40 AM"));
        }
    }
}

#[test]
fn title_chrome_selects_text_and_exposes_a_named_editable_field() {
    let mut engine = TextEngine::default();
    let mut source = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new(
            "Header".into(),
            Format {
                font_size: Some(8.0),
                line_spacing: Some(20.751953),
                ..Default::default()
            },
        )])
        .unwrap(),
        468.0,
        [36.0, 14.4],
    )
    .unwrap()
    .snapshot();
    source.title = true;
    source.min_width = Some(162.0);
    source.layout.max_width = None;
    source.layout.width_set_by_user = None;
    source.layout.max_height = Some(21.6);
    let outline = TextOutline::from_outline(&mut engine, &source, &Default::default()).unwrap();
    let editor = CanvasEditor::from_text_outlines(vec![outline], Default::default(), None).unwrap();
    assert_eq!(editor.active_outline().bounds().width(), 162.0);
    assert_eq!(editor.active_outline().snapshot().min_width, Some(162.0));
    let (native_rect, _) = outline_chrome(editor.active_outline(), 0.75);
    // Unlike a body outline's, the title's box scales with zoom (OneNote 2010 at 200%).
    assert_eq!(
        outline_chrome(editor.active_outline(), 0.375).0,
        native_rect
    );
    for (axis, (actual, expected)) in native_rect
        .iter()
        .zip([85.0, 98.0, 315.0, 134.0])
        .enumerate()
    {
        let origin = if axis % 2 == 0 { 48.0 } else { 83.0 };
        assert!((actual / 0.75 + origin - expected).abs() < 1.5);
    }
    let id = editor.active_outline().id;
    let (rect, _) = outline_chrome(editor.active_outline(), 1.0);
    for point in [
        [rect[0] + 1.0, rect[1] + 1.0],
        [rect[2] - 1.0, rect[1] + 1.0],
        [40.0, 20.0],
    ] {
        assert!(
            matches!(page_hit_test(&editor, None, point, 1.0), Some(Hit::Text { id: hit, .. }) if hit == id)
        );
    }
    let primitives = page_primitives(&editor, None, None, None, paint(false, 1.0, 1.0)).unwrap();
    let borders = primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::RoundedRect { stroke, .. } => Some(*stroke),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(borders, [Some(Stroke::Dashed(1.0))]);
    assert!(primitives.iter().any(|primitive| matches!(primitive, Primitive::RoundedRect { rect, radius, stroke: Some(Stroke::Dashed(_)), .. } if radius[0] == 4.5 && radius[1] * 2.0 == rect[3] - rect[1])));
    let mut access = accessibility::Accessibility::default();
    let update = access
        .update(
            &editor,
            None,
            Viewport {
                size: [800, 600],
                scale: 1.0,
                origin: [0.0; 2],
            },
            "Header",
            None,
            None,
        )
        .unwrap();
    assert!(update.nodes.iter().any(|(_, node)| node.role()
        == accesskit::Role::MultilineTextInput
        && node.label() == Some("Page title")));
}

#[test]
fn title_frame_shows_unfocused_and_its_pen_scales_only_when_zoomed_in() {
    let mut engine = TextEngine::default();
    let mut outline = |text: &str, origin, title| {
        let mut source = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new(text.into(), Format::default())]).unwrap(),
            468.0,
            origin,
        )
        .unwrap()
        .snapshot();
        source.title = title;
        TextOutline::from_outline(&mut engine, &source, &Default::default()).unwrap()
    };
    let title = outline("Title", [36.0, 14.4], true);
    let body = outline("Body", [36.0, 120.0], false);
    let body_id = body.id;
    let mut editor =
        CanvasEditor::from_text_outlines(vec![title, body], Default::default(), None).unwrap();
    editor.focus_outline(body_id).unwrap();
    // OneNote 2010: a 4 px dash period at 100% and below, 4 px times the zoom above it.
    for (zoom, pen) in [(0.5, 1.5), (1.0, 0.75), (2.0, 0.75), (3.0, 0.75)] {
        let scale = zoom * 96.0 / 72.0;
        let primitives =
            page_primitives(&editor, None, None, None, paint(false, scale, 1.0)).unwrap();
        let dashed = primitives
            .iter()
            .filter_map(|primitive| match primitive {
                Primitive::RoundedRect {
                    stroke: Some(Stroke::Dashed(width)),
                    ..
                } => Some(*width),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(
            matches!(dashed[..], [width] if (width - pen).abs() < 1e-5),
            "zoom {zoom}: {dashed:?}"
        );
    }
}

#[test]
fn provisional_lines_share_one_drawn_hit_tested_and_accessible_outline() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("body".into(), Format::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    let id = editor.active_outline().id;
    for _ in 0..2 {
        editor
            .move_selection(&mut engine, Movement::Down, false)
            .unwrap();
    }
    let primitives = page_primitives(&editor, None, None, None, paint(true, 1.0, 1.0)).unwrap();
    assert_eq!(
        primitives
            .iter()
            .filter(|p| matches!(p, Primitive::Text { .. }))
            .count(),
        3
    );
    let caret = editor.caret(1.0).unwrap();
    assert!(
        matches!(page_hit_test(&editor, None, [caret.x0 as f32, ((caret.y0 + caret.y1) * 0.5) as f32], 1.0), Some(Hit::Text { id: hit, .. }) if hit == id)
    );
    let mut access = accessibility::Accessibility::default();
    let update = access
        .update(
            &editor,
            None,
            Viewport {
                size: [800, 600],
                scale: 1.0,
                origin: [0.0; 2],
            },
            "Test",
            None,
            None,
        )
        .unwrap();
    let fields = update
        .nodes
        .iter()
        .filter(|(_, node)| node.role() == accesskit::Role::MultilineTextInput)
        .collect::<Vec<_>>();
    assert_eq!(fields.len(), 1);
    assert_eq!(fields[0].1.value(), Some("body\n\n"));
    assert_eq!(editor.outlines()[0].document().nodes().len(), 1);
    editor
        .place_caret(&mut engine, [300.0, 200.0], 240.0)
        .unwrap();
    assert_eq!(
        editor
            .visible_outlines()
            .next()
            .unwrap()
            .document()
            .nodes()
            .len(),
        1
    );
}

#[test]
fn blank_caret_and_composition_have_accessible_text_without_outline_chrome() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new(String::new(), Default::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    editor
        .place_caret(&mut engine, [120.0, 100.0], 240.0)
        .unwrap();
    let mut access = accessibility::Accessibility::default();
    let viewport = Viewport {
        size: [1000, 720],
        scale: 96.0 / 72.0,
        origin: [48.0; 2],
    };
    let initial = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    let id = editor.active_outline().id;
    assert_eq!(access.outline_for_node(initial.focus), Some(id));
    let rectangles = |editor: &CanvasEditor| {
        page_primitives(editor, None, None, None, paint(true, viewport.scale, 1.0))
            .unwrap()
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Rect { rect, color } | Primitive::RoundedRect { rect, color, .. } => {
                    Some((rect, color))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(rectangles(&editor).len(), 1);
    assert_eq!(
        page_hit_test(&editor, None, [120.0, 100.0], 1.0 / viewport.scale),
        None
    );
    editor.compose(&mut engine, "に".into(), 1..1).unwrap();
    assert!(editor.outlines().is_empty());
    assert!(
        rectangles(&editor)
            .iter()
            .all(|(_, color)| *color == [0.0, 0.0, 0.0, 1.0] || *color == COLORS.caret)
    );
    let preedit = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    assert_eq!(preedit.focus, initial.focus);
    editor.commit_text(&mut engine, "日本".into()).unwrap();
    let committed = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    assert_eq!(committed.focus, initial.focus);
    assert!(rectangles(&editor).len() > 1);
    editor.undo(&mut engine).unwrap();
    assert!(editor.outlines().is_empty());
    assert_eq!(rectangles(&editor).len(), 1);
    let empty = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    assert_eq!(empty.focus, committed.focus);
    assert_eq!(access.outline_for_node(committed.focus), Some(id));
    editor.redo(&mut engine).unwrap();
    editor.select_all().unwrap();
    editor.delete(&mut engine, true).unwrap();
    assert!(editor.outlines().is_empty());
    assert_eq!(rectangles(&editor).len(), 1);
    let retired = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    assert_eq!(retired.focus, committed.focus);
    assert_eq!(access.outline_for_node(retired.focus), Some(id));
    editor.undo(&mut engine).unwrap();
    assert!(rectangles(&editor).len() > 1);
    let restored = access
        .update(&editor, None, viewport, "Page", None, None)
        .unwrap();
    assert_eq!(restored.focus, committed.focus);
    editor.redo(&mut engine).unwrap();
    assert_eq!(rectangles(&editor).len(), 1);
    assert!(editor.caret_outline().unwrap().is_empty());
    editor.insert(&mut engine, "   ").unwrap();
    assert!(editor.caret_outline().unwrap().is_empty());
    editor.compose(&mut engine, "に".into(), 1..1).unwrap();
    assert!(!editor.caret_outline().unwrap().is_empty());
}

#[test]
#[ignore = "requires CANVAS_TEST_SECTION and CANVAS_TEST_PAGE private fixture inputs"]
fn imported_page_widgets_and_accessibility_follow_edits() {
    let bytes = std::fs::read(std::env::var_os("CANVAS_TEST_SECTION").unwrap()).unwrap();
    let page = {
        let store = onestore::Store::parse(&bytes).unwrap();
        let index = onestore::RevisionIndex::parse(&store).unwrap();
        Page::from_document(
            &onestore::document::Document::parse(&index).unwrap(),
            &std::env::var("CANVAS_TEST_PAGE").unwrap(),
        )
        .unwrap()
    };
    drop(bytes);
    let mut engine = TextEngine::default();
    if let Some(font) = std::env::var_os("CANVAS_TEST_SUBSTITUTE") {
        engine
            .register_substitute(parley::fontique::Blob::new(Arc::new(
                std::fs::read(font).unwrap(),
            )))
            .unwrap();
    }
    let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
    let scene = (scene, [0.0; 2]);
    let viewport = Viewport {
        size: [1000, 720],
        scale: 96.0 / 72.0,
        origin: [48.0; 2],
    };
    let mut tag_hits = 0;
    for outline in editor.outlines() {
        for (_, paragraph) in outline.layouts() {
            for tag in &paragraph.tags {
                let point = [
                    outline.origin()[0] + tag.origin[0] + crate::outline::ParagraphTag::SIZE / 2.0,
                    outline.origin()[1]
                        + paragraph.origin[1]
                        + tag.origin[1]
                        + crate::outline::ParagraphTag::SIZE / 2.0,
                ];
                assert!(
                    matches!(page_hit_test(&editor, Some(&scene), point, 1.0), Some(Hit::Text { id, .. }) if id == outline.id),
                    "Tag center {point:?} did not target outline {:?}",
                    outline.id
                );
                tag_hits += 1;
            }
        }
    }
    eprintln!("Verified {tag_hits} imported tag-gutter targets");
    let mut access = accessibility::Accessibility::default();
    let mut tree = None;
    let tags = |editor: &CanvasEditor| {
        page_primitives(
            editor,
            Some(&scene),
            None,
            None,
            paint(true, viewport.scale, 1.0),
        )
        .unwrap()
        .into_iter()
        .filter_map(|primitive| match primitive {
            Primitive::Icon {
                sources,
                origin,
                tint,
                ..
            } => Some(((sources, tint[3]), origin)),
            _ => None,
        })
        .collect::<Vec<_>>()
    };
    let original_tags = tags(&editor);
    let ids: Vec<_> = editor.outlines().iter().map(|outline| outline.id).collect();
    for id in ids {
        editor.focus_outline(id).unwrap();
        let original = editor.active_outline().document().clone();
        editor
            .select(
                [crate::document::TextPosition {
                    paragraph: 0,
                    offset: 0,
                }; 2]
                    .into(),
            )
            .unwrap();
        let before = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        let before = accessibility::tests::apply(&mut tree, before)
            .focus()
            .unwrap()
            .document_range()
            .text();
        editor.insert(&mut engine, "CANVAS CHECK ").unwrap();
        {
            let update = access
                .update(&editor, None, viewport, "Test", None, None)
                .unwrap();
            assert_eq!(
                accessibility::tests::apply(&mut tree, update)
                    .focus()
                    .unwrap()
                    .document_range()
                    .text(),
                format!("CANVAS CHECK {before}")
            );
            let primitives = page_primitives(
                &editor,
                Some(&scene),
                None,
                None,
                paint(true, viewport.scale, 1.0),
            )
            .unwrap();
            assert!(!primitives.is_empty());
            let edited_tags = tags(&editor);
            assert_eq!(edited_tags.len(), original_tags.len());
            for ((edited, _), (original, _)) in edited_tags.iter().zip(&original_tags) {
                assert_eq!(edited, original);
            }
        }
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.active_outline().document(), &original);
        assert_eq!(tags(&editor), original_tags);
        let update = access
            .update(&editor, None, viewport, "Test", None, None)
            .unwrap();
        assert_eq!(
            accessibility::tests::apply(&mut tree, update)
                .focus()
                .unwrap()
                .document_range()
                .text(),
            before
        );
    }
    eprintln!(
        "Rendered and restored {} imported outline widgets and accessibility fields",
        editor.outlines().len()
    );
}

#[test]
fn extension_hits_yield_to_objects_and_keep_their_source_coordinate_frame() {
    let mut engine = TextEngine::default();
    let upper = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Upper".into(), Default::default())]).unwrap(),
        240.0,
        [36.0, 90.0],
    )
    .unwrap();
    let bottom = upper.bounds().y1 as f32;
    let id = upper.id;
    let lower = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Lower".into(), Default::default())]).unwrap(),
        240.0,
        [36.0, bottom + 20.0],
    )
    .unwrap();
    let lower_id = lower.id;
    let source = upper.snapshot();
    let mut editor =
        CanvasEditor::from_text_outlines(vec![lower, upper], Default::default(), None).unwrap();
    assert!(
        matches!(page_hit_test(&editor, None, [60.0, bottom + 22.0], 0.75), Some(Hit::Text { id, .. }) if id == lower_id)
    );
    editor.move_outline(lower_id, [600.0, 500.0]).unwrap();
    for pixel in [0.375, 0.75, 1.5] {
        let hit = page_hit_test(&editor, None, [60.0, bottom + 26.0], pixel).unwrap();
        assert!(
            matches!(hit, Hit::Text { id: hit_id, point } if hit_id == id && (point[1] - (bottom + 26.0 - 90.0)).abs() < 0.001)
        );
        assert_eq!(
            page_hit_test(&editor, None, [60.0, bottom + 30.0], pixel),
            None
        );
    }
    let page = onestore::page::Page {
        identity: None,
        title: String::new(),
        created: None,
        margin_origin: [0.0; 2],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects: vec![onestore::page::PageObject::Outline(source)],
    };
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let offset = [50.0, 300.0];
    assert!(
        matches!(page_hit_test(&editor, Some(&(scene, offset)), [60.0 + offset[0], bottom + 26.0 + offset[1]], 0.75), Some(Hit::Text { id: hit_id, .. }) if hit_id == id)
    );
}

#[test]
fn chrome_follows_focus_hover_and_drag_without_changing_hit_geometry() {
    let mut engine = TextEngine::default();
    let outlines = [[0.0, 0.0], [220.0, 0.0]]
        .into_iter()
        .map(|origin| {
            TextOutline::new(
                &mut engine,
                TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
                100.0,
                origin,
            )
            .unwrap()
        })
        .collect();
    let editor = CanvasEditor::from_text_outlines(outlines, Default::default(), None).unwrap();
    let second = editor.outlines()[1].id;
    let borders = |feedback| {
        page_primitives(&editor, None, feedback, None, paint(false, 1.0, 1.0))
            .unwrap()
            .into_iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect {
                    rect,
                    stroke: Some(Stroke::Solid(1.0)),
                    ..
                } => Some(rect),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(borders(None).len(), 1);
    let hovered = borders(Some(PointerFeedback::Hover(second)));
    assert_eq!(hovered.len(), 2);
    let moved = borders(Some(PointerFeedback::Move(second, [20.0, 50.0])));
    assert_eq!(moved[1][0] - hovered[1][0], 20.0);
    assert_eq!(moved[1][1] - hovered[1][1], 50.0);
    assert!(
        matches!(page_hit_test(&editor, None, [225.0, 5.0], 1.0), Some(Hit::Text { id, .. }) if id == second)
    );
}

#[test]
fn a_covered_width_handle_stays_reachable_beside_the_outline_above_it() {
    let mut engine = TextEngine::default();
    let pixel = 0.75;
    let mut first = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
        100.0,
        [36.0, 68.4],
    )
    .unwrap();
    let editor =
        CanvasEditor::from_text_outlines(vec![first.clone()], Default::default(), None).unwrap();
    first = editor.preview_resize(&mut engine, 226.5).unwrap();
    let (frame, body_top) = outline_chrome(&first, pixel);
    // The casual night page: the next outline starts 7.5 pt right of the first one.
    let second = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
        100.0,
        [first.bounds().x1 as f32 + 7.5, 68.4],
    )
    .unwrap();
    let (first_id, second_id) = (first.id, second.id);
    let editor =
        CanvasEditor::from_text_outlines(vec![first, second], Default::default(), None).unwrap();
    let header = (frame[1] + body_top) / 2.0;
    let hit = |x| page_hit_test(&editor, None, [x, header], pixel);
    assert!(matches!(hit(frame[2] - 12.0 * pixel), Some(Hit::Resize { id, .. }) if id == first_id));
    assert!(matches!(hit(frame[2] - 2.0 * pixel), Some(Hit::Handle { id, .. }) if id == second_id));
    let body = |x| page_hit_test(&editor, None, [x, body_top + 20.0], pixel);
    assert!(matches!(body(frame[2] - 4.0 * pixel), Some(Hit::Resize { id, .. }) if id == first_id));
    assert!(matches!(body(frame[2] - 8.0 * pixel), Some(Hit::Text { id, .. }) if id == second_id));
    assert!(matches!(body(frame[2] - 12.0 * pixel), Some(Hit::Text { id, .. }) if id == first_id));
}

#[test]
fn a_picture_in_an_outline_takes_the_click_over_its_text() {
    use onestore::page::{Image, Page, PageObject, ParagraphContent};
    let mut engine = TextEngine::default();
    let mut source = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![
            Paragraph::new("Before".into(), Default::default()),
            Paragraph::new("After".into(), Default::default()),
        ])
        .unwrap(),
        240.0,
        [36.0, 36.0],
    )
    .unwrap()
    .snapshot();
    let mut picture = source.paragraphs[0].clone();
    picture.id = onestore::page::text::new_id().unwrap();
    let id = onestore::page::text::new_id().unwrap();
    picture.content = ParagraphContent::Image(Image {
        size: Some([40.0, 30.0]),
        id,
        layout: Default::default(),
        bytes: None,
        display: None,
        alt: None,
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    });
    source.paragraphs.insert(1, picture);
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![PageObject::Outline(source)],
        },
        &mut engine,
    )
    .unwrap();
    let (origin, size) = editor.image_placement(id).unwrap();
    let center = [origin[0] + size[0] / 2.0, origin[1] + size[1] / 2.0];
    assert_eq!(
        page_hit_test(&editor, None, center, 1.0),
        Some(Hit::Image { id, handle: [0, 0] })
    );
    assert!(matches!(
        page_hit_test(&editor, None, [origin[0] + 2.0, origin[1] - 4.0], 1.0),
        Some(Hit::Text { .. })
    ));
}

#[test]
fn picture_handles_resize_as_onenote_does() {
    // Native drags of the 333 x 200.1 pt mockup on "av: casual night in the trees".
    let (origin, size) = ([468.0, 86.4], [333.0, 200.1]);
    let (corner_origin, corner) = resize_image(origin, size, [1, -1], [-75.0, 30.75]);
    assert!((corner[0] - 281.827).abs() < 0.01 && (corner[1] - 169.351).abs() < 0.01);
    assert!((corner_origin[1] - 117.15).abs() < 0.01 && corner_origin[0] == 468.0);
    assert_eq!(
        resize_image(origin, size, [1, 0], [46.5, 10.0]),
        (origin, [379.5, 200.1])
    );
    let (left_origin, left) = resize_image(origin, size, [-1, 0], [400.0, 0.0]);
    assert_eq!(left, [1.0, 200.1]);
    assert_eq!(left_origin[0], 468.0 + 333.0 - 1.0);
    let pixel = 0.75;
    let rect = image_rect(origin, size);
    assert_eq!(
        image_handle_at(rect, pixel, [468.0 - 3.75, 86.4 - 3.75], 0.0),
        Some([-1, -1])
    );
    assert_eq!(
        image_handle_at(rect, pixel, [468.0 + 166.5, 286.5 + 3.75], 0.0),
        Some([0, 1])
    );
    assert_eq!(image_handle_at(rect, pixel, [600.0, 150.0], 0.0), None);
}

const DEFAULT_MARGIN: [f32; 2] = [36.0, 14.4];

#[test]
fn native_grid_follows_the_page_margin_origin() {
    // "av: casual night in the trees": margin 36.75; a 22.5 x 15 pt drag lands one cell on.
    let margin = [36.75, 14.4];
    let result = snap_to_grid([468.75 + 22.5, 86.4 + 15.0], margin);
    assert!((result[0] - 486.75).abs() < 0.0001);
    assert!((result[1] - 104.4).abs() < 0.0001);
}

#[test]
fn native_grid_matches_drag_offsets_midpoints_and_zoom() {
    for (pixels, points) in [
        (5.0, 0.0),
        (6.0, 0.0),
        (7.0, 0.0),
        (8.0, 0.0),
        (11.0, 0.0),
        (12.0, 9.0),
        (13.0, 18.0),
        (17.0, 18.0),
        (18.0, 18.0),
        (19.0, 18.0),
        (25.0, 18.0),
        (37.0, 36.0),
        (-6.0, 0.0),
        (-7.0, 0.0),
        (-12.0, -9.0),
        (-13.0, -18.0),
        (-19.0, -18.0),
    ] {
        let origin = [486.0, 230.40001];
        let proposed = origin.map(|v| v + pixels * 72.0 / 96.0);
        let result = snap_to_grid(proposed, DEFAULT_MARGIN);
        for axis in 0..2 {
            assert!((result[axis] - origin[axis] - points).abs() < 0.00004);
        }
    }
    for pixels in [6.0, 7.0, 12.0, 13.0, 19.0] {
        let result = snap_to_grid(
            [491.25 + pixels * 0.75, 235.65 + pixels * 0.75],
            DEFAULT_MARGIN,
        );
        assert!((result[0] - 504.0).abs() < 0.00004);
        assert!((result[1] - 248.4).abs() < 0.00004);
    }
    for (scale, delta, expected) in [
        (96.0 / 72.0, [27.0, 31.0], [54.0, 104.4]),
        (96.0 / 72.0, [13.0, 17.0], [54.0, 104.4]),
        (192.0 / 72.0, [13.0, 17.0], [36.0, 104.4]),
    ] {
        for dpr in [1.0, 2.0] {
            let origin = [36.0, 90.0];
            let proposed =
                std::array::from_fn(|axis| origin[axis] + delta[axis] * dpr / (scale * dpr));
            let result = snap_to_grid(proposed, DEFAULT_MARGIN);
            for axis in 0..2 {
                assert!((result[axis] - expected[axis]).abs() < 0.00004);
            }
        }
    }
    for point in [[9.0, 5.4], [-9.0, -12.6], [423.0, 239.4]] {
        assert_eq!(snap_to_grid(point, DEFAULT_MARGIN), point);
    }
}

#[test]
fn native_fresh_click_grid_uses_screen_offset_at_both_zoom_levels() {
    for (x, y, scale, expected) in [
        (900.0, 500.0, 96.0 / 72.0, [639.0, 302.4]),
        (901.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
        (906.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
        (913.0, 500.0, 96.0 / 72.0, [648.0, 302.4]),
        (900.0, 501.0, 96.0 / 72.0, [639.0, 302.4]),
        (900.0, 506.0, 96.0 / 72.0, [639.0, 320.4]),
        (900.0, 520.0, 96.0 / 72.0, [639.0, 320.4]),
        (900.0, 500.0, 192.0 / 72.0, [324.0, 158.4]),
    ] {
        for dpr in [1.0, 2.0] {
            let viewport = Viewport {
                size: [(1600.0 * dpr) as u32, (900.0 * dpr) as u32],
                origin: [48.0 * dpr, 83.0 * dpr],
                scale: scale * dpr,
            };
            let point = viewport.document_point([x * dpr, y * dpr]);
            let result = snap_to_grid(
                [point[0], point[1] - 7.0 * dpr / viewport.scale],
                DEFAULT_MARGIN,
            );
            for axis in 0..2 {
                assert!((result[axis] - expected[axis]).abs() < 0.00004);
            }
        }
    }
}

#[test]
fn table_glyphs_highlights_and_selection_share_cell_paint_bounds() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new(
            "M".into(),
            Format {
                font_size: Some(130.0),
                highlight: Some(0xffff),
                ..Default::default()
            },
        )])
        .unwrap(),
        180.0,
    )
    .unwrap();
    editor
        .move_selection(&mut engine, Movement::LineEnd, false)
        .unwrap();
    editor.tab(&mut engine, false).unwrap();
    editor.insert(&mut engine, "R").unwrap();
    // Columns dragged narrower than the glyphs they hold.
    let table = editor.active_outline().shaped().tables[0].id;
    for column in 0..2 {
        editor
            .resize_column(&mut engine, table, column, 37.11)
            .unwrap();
    }
    editor.tab(&mut engine, true).unwrap();
    let mut primitives = Vec::new();
    append_outline(
        Some(&editor),
        false,
        editor.active_outline(),
        [24.0, 48.0],
        Paint {
            caret: 0.0,
            scale: 1.0,
            device_origin: [0.0; 2],
            pixel: 1.0,
            colors: COLORS,
            visible: [f32::NEG_INFINITY, f32::INFINITY],
            chrome: true,
            found: &[],
            played: None,
            spelling: None,
            tag_art: &NO_ART,
        },
        &mut primitives,
    )
    .unwrap();
    let bounds = editor.active_outline().shaped().tables[0]
        .cells
        .iter()
        .map(|cell| {
            let [left, top, right, bottom] = cell.text_bounds();
            [left + 24.0, top + 48.0, right + 24.0, bottom + 48.0]
        })
        .collect::<Vec<_>>();
    let text = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Text { clip, .. } => Some(clip.unwrap()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(text, bounds);
    let highlights = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Rect { rect, color } if *color == crate::gpu::colorref(0xffff) => {
                Some(*rect)
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(highlights.len(), 2);
    for (rect, bounds) in highlights.iter().zip(&bounds) {
        assert_eq!(rect[2], bounds[2]);
        assert!(rect[0] >= bounds[0] && rect[1] >= bounds[1] && rect[3] <= bounds[3]);
    }
    let selection = primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::Rect { rect, color } if *color == COLORS.selection => Some(*rect),
            _ => None,
        })
        .unwrap();
    assert_eq!(selection[2], bounds[0][2]);
    assert!(editor.caret(1.0).unwrap().x0 + 24.0 > f64::from(bounds[0][2]));
}

#[test]
fn editable_tables_paint_borders_before_selection_and_cell_text() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Left".into(), Format::default())]).unwrap(),
        180.0,
    )
    .unwrap();
    editor
        .move_selection(&mut engine, Movement::LineEnd, false)
        .unwrap();
    editor.tab(&mut engine, false).unwrap();
    editor.insert(&mut engine, "Right").unwrap();
    editor.tab(&mut engine, true).unwrap();
    let mut primitives = Vec::new();
    append_outline(
        Some(&editor),
        false,
        editor.active_outline(),
        [24.0, 48.0],
        Paint {
            caret: 0.0,
            scale: 1.0,
            device_origin: [0.0; 2],
            pixel: 1.0,
            colors: COLORS,
            visible: [f32::NEG_INFINITY, f32::INFINITY],
            chrome: true,
            found: &[],
            played: None,
            spelling: None,
            tag_art: &NO_ART,
        },
        &mut primitives,
    )
    .unwrap();
    let color = crate::gpu::colorref(0x00a3a3a3);
    assert!(matches!(primitives[0], Primitive::RoundedRect {
        stroke: Some(draw::Stroke::Solid(0.75)), color: actual, ..
    } if actual == color));
    assert!(matches!(primitives[1], Primitive::Rect { color: actual, .. } if actual == color));
    assert!(matches!(primitives[2], Primitive::Rect { color, .. } if color == COLORS.selection));
    let painted = primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::Text { text, origin, .. } => Some((painted_layout(*text).id(), *origin)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let expected = editor
        .active_outline()
        .layouts()
        .map(|(_, paragraph)| {
            (
                paragraph.text.id(),
                [paragraph.origin[0] + 24.0, paragraph.origin[1] + 48.0],
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(painted, expected);
    assert_eq!(painted.len(), 2);
}

#[test]
fn resize_preview_paints_reflowed_text_without_changing_the_editor() {
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new(
            "A paragraph with enough words to wrap when the resize handle moves inward.".into(),
            Format::default(),
        )])
        .unwrap(),
        240.0,
    )
    .unwrap();
    let original = editor.active_outline().snapshot();
    let original_layout = editor
        .active_outline()
        .paragraph_layout(0)
        .unwrap()
        .text
        .id();
    let resized = editor.preview_resize(&mut engine, 72.0).unwrap();
    let primitives = page_primitives(
        &editor,
        None,
        Some(PointerFeedback::Resize(&resized)),
        None,
        paint(false, 96.0 / 72.0, 1.0),
    )
    .unwrap();
    let painted = primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::Text { text, .. } => Some(painted_layout(*text)),
            _ => None,
        })
        .unwrap();
    assert_eq!(painted.id(), resized.paragraph_layout(0).unwrap().text.id());
    assert!(
        painted.lines().count()
            > editor
                .active_outline()
                .paragraph_layout(0)
                .unwrap()
                .text
                .lines()
                .count()
    );
    assert_eq!(editor.active_outline().layout(), &original.layout);
    assert_eq!(
        editor
            .active_outline()
            .paragraph_layout(0)
            .unwrap()
            .text
            .id(),
        original_layout
    );
    assert!(primitives.iter().any(
        |primitive| matches!(primitive, Primitive::RoundedRect { stroke, .. } if stroke.is_some())
    ));
}

#[test]
fn native_chrome_geometry_preserves_text_and_matches_hit_regions_across_zoom_and_dpi() {
    let mut engine = TextEngine::default();
    let format = Format {
        font: Some("Arial".into()),
        font_size: Some(11.0),
        ..Default::default()
    };
    let mut editor = CanvasEditor::new(&mut engine, TextDocument::new(vec![
        Paragraph::new("HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789 HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789 HAMBURGEFONTS abcdefghijklmnopqrstuvwxyz 0123456789".into(), format.clone()),
        Paragraph::new("Second paragraph abcdefghijklmnopqrstuvwxyz.".into(), format),
    ]).unwrap(), 220.0).unwrap();
    let id = editor.active_outline().id;
    editor.move_outline(id, [36.0, 90.0]).unwrap();
    let document = editor.active_outline().document().clone();
    let layout_ids: Vec<_> = editor
        .active_outline()
        .layouts()
        .map(|(_, p)| p.text.id())
        .collect();
    for (zoom, expected) in [
        (0.5, [62.0, 134.0, 223.0, 219.0]),
        (1.0, [81.0, 189.0, 394.0, 348.0]),
        (1.5, [101.0, 245.0, 565.0, 479.0]),
        (2.0, [120.0, 300.0, 736.0, 608.0]),
    ] {
        for dpr in [1.0, 2.0] {
            let scale = zoom * (96.0 / 72.0) * dpr;
            let pixel = dpr / scale;
            let (frame, body_top) = outline_chrome(editor.active_outline(), pixel);
            for (index, coordinate) in frame.iter().enumerate() {
                let origin = if index % 2 == 0 { 48.0 } else { 83.0 };
                let screen = coordinate * scale / dpr + origin;
                assert!(
                    (screen - expected[index]).abs() <= 1.5,
                    "zoom={zoom} dpr={dpr} edge={index}: {screen} vs {}",
                    expected[index]
                );
            }
            let header = [36.0, (frame[1] + body_top) / 2.0];
            assert!(
                matches!(page_hit_test(&editor, None, header, pixel), Some(Hit::Handle {id: hit, ..}) if hit == id)
            );
            assert!(matches!(
                page_hit_test(&editor, None, [frame[2] - 4.0 * pixel, header[1]], pixel),
                Some(Hit::Resize { id: hit, .. }) if hit == id
            ));
            let padding = [frame[0] + pixel, 90.0];
            assert!(
                matches!(page_hit_test(&editor, None, padding, pixel), Some(Hit::Text {id: hit, ..}) if hit == id)
            );
            assert_eq!(
                page_hit_test(&editor, None, [frame[0] - pixel, 90.0], pixel),
                None
            );
            let primitives =
                page_primitives(&editor, None, None, None, paint(false, scale, dpr)).unwrap();
            assert!(
                matches!(&primitives[0], Primitive::RoundedRect {rect, ..} if *rect == [frame[0], frame[1], frame[2], body_top])
            );
            assert!(
                primitives
                    .iter()
                    .any(|p| matches!(p, Primitive::Text {origin, ..} if *origin == [36.0, 90.0]))
            );
        }
    }
    assert_eq!(editor.active_outline().document(), &document);
    assert_eq!(
        editor
            .active_outline()
            .layouts()
            .map(|(_, p)| p.text.id())
            .collect::<Vec<_>>(),
        layout_ids
    );
}

#[test]
fn a_later_text_body_takes_priority_over_an_earlier_drag_handle() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("first".into(), Default::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    let first = editor.active_outline().id;
    editor.move_outline(first, [0.0, 20.0]).unwrap();
    assert_eq!(
        page_hit_test(&editor, None, [5.0, 12.0], 1.0),
        Some(Hit::Handle {
            id: first,
            grab: [5.0, -8.0]
        })
    );
    let second = editor
        .create_outline(&mut engine, [0.0, 10.0], 240.0)
        .unwrap();
    assert_eq!(
        page_hit_test(&editor, None, [5.0, 12.0], 1.0),
        Some(Hit::Text {
            id: second,
            point: [5.0, 2.0]
        })
    );
    editor.undo(&mut engine).unwrap();
    assert_eq!(
        page_hit_test(&editor, None, [5.0, 12.0], 1.0),
        Some(Hit::Handle {
            id: first,
            grab: [5.0, -8.0]
        })
    );
}

#[test]
fn overlapping_objects_follow_paint_order_through_creation_movement_and_undo() {
    use onestore::page::{Outline, PageObject, Unsupported};
    for readonly_on_top in [false, true] {
        let mut engine = TextEngine::default();
        let document = TextDocument::new(vec![Paragraph::new(
            "Imported text".into(),
            Default::default(),
        )])
        .unwrap();
        let id = onestore::ExGuid {
            guid: [5; 16],
            n: 1,
        };
        let mut objects = vec![
            PageObject::Outline(Outline {
                id,
                title: false,
                min_width: None,
                layout: onestore::document::Layout {
                    x: Some(30.0),
                    y: Some(40.0),
                    max_width: Some(180.0),
                    ..Default::default()
                },
                indents: vec![18.0, 0.0],
                paragraphs: document.nodes().to_vec(),
                unsupported: Vec::new(),
            }),
            PageObject::Unsupported(Unsupported {
                id: Default::default(),
                jcid: 0xdead,
                layout: onestore::document::Layout {
                    x: Some(20.0),
                    y: Some(20.0),
                    max_width: Some(200.0),
                    max_height: Some(100.0),
                    ..Default::default()
                },
            }),
        ];
        if !readonly_on_top {
            objects.reverse();
        }
        let page = Page {
            identity: None,
            created: None,
            title: String::new(),
            margin_origin: [0.0; 2],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects,
        };
        let (scene, mut editor) = PageScene::from_page(page, &mut engine).unwrap();
        let scene = (scene, [0.0; 2]);
        let imported_layout = editor
            .active_outline()
            .layouts()
            .next()
            .unwrap()
            .1
            .text
            .id();
        let texts: Vec<_> =
            page_primitives(&editor, Some(&scene), None, None, paint(false, 1.0, 1.0))
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text { text, .. } => Some(painted_layout(text).id()),
                    _ => None,
                })
                .collect();
        assert_eq!(texts.len(), 2);
        assert_eq!(texts[usize::from(!readonly_on_top)], imported_layout);
        let expected = if readonly_on_top {
            Hit::ReadOnly(0)
        } else {
            Hit::Text {
                id,
                point: [10.0, 5.0],
            }
        };
        assert_eq!(
            page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
            Some(expected)
        );
        let header = page_hit_test(&editor, Some(&scene), [40.0, 32.0], 1.0);
        if readonly_on_top {
            assert_eq!(header, Some(Hit::ReadOnly(0)));
        } else {
            assert_eq!(
                header,
                Some(Hit::Handle {
                    id,
                    grab: [10.0, -8.0]
                })
            );
        }
        let annotation = editor
            .create_outline(&mut engine, [30.0, 40.0], 180.0)
            .unwrap();
        assert_eq!(
            page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
            Some(Hit::Text {
                id: annotation,
                point: [10.0, 5.0]
            })
        );
        assert_eq!(
            page_hit_test(&editor, Some(&scene), [40.0, 32.0], 1.0),
            Some(Hit::Handle {
                id: annotation,
                grab: [10.0, -8.0]
            })
        );
        let texts: Vec<_> =
            page_primitives(&editor, Some(&scene), None, None, paint(false, 1.0, 1.0))
                .unwrap()
                .into_iter()
                .filter_map(|p| match p {
                    Primitive::Text { text, .. } => Some(painted_layout(text).id()),
                    _ => None,
                })
                .collect();
        assert_eq!(
            *texts.last().unwrap(),
            editor
                .active_outline()
                .layouts()
                .next()
                .unwrap()
                .1
                .text
                .id()
        );
        editor.move_outline(annotation, [500.0, 500.0]).unwrap();
        assert!(
            !matches!(page_hit_test(&editor, Some(&scene), [40.0,45.0], 1.0), Some(Hit::Text { id, .. }) if id == annotation)
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(
            page_hit_test(&editor, Some(&scene), [40.0, 45.0], 1.0),
            Some(Hit::Text {
                id: annotation,
                point: [10.0, 5.0]
            })
        );
        editor.undo(&mut engine).unwrap();
        assert_eq!(editor.outlines().len(), 1);
        assert_eq!(editor.active_outline().document(), &document);
        assert_eq!(
            page_hit_test(&editor, Some(&scene), [10.0, 10.0], 1.0),
            None
        );
    }
}

#[test]
fn read_only_focus_accepts_navigation_without_edit_shortcuts() {
    for modifiers in [
        Modifiers::default(),
        Modifiers {
            shift: true,
            ..Modifiers::default()
        },
        Modifiers {
            option: true,
            ..Modifiers::default()
        },
        Modifiers {
            command: true,
            ..Modifiers::default()
        },
        Modifiers {
            command: true,
            shift: true,
            ..Modifiers::default()
        },
        Modifiers {
            command: true,
            option: true,
            ..Modifiers::default()
        },
    ] {
        for key in [
            Key::Character("a".into()),
            Key::Character("v".into()),
            Key::Character("c".into()),
            Key::Character("x".into()),
            Key::Character("z".into()),
            Key::Character("🌳".into()),
            Key::Named(NamedKey::Backspace),
            Key::Named(NamedKey::Delete),
            Key::Named(NamedKey::Enter),
            Key::Named(NamedKey::ArrowLeft),
            Key::Named(NamedKey::ArrowRight),
            Key::Named(NamedKey::ArrowUp),
            Key::Named(NamedKey::ArrowDown),
        ] {
            assert!(
                !read_only_shortcut(&key, modifiers),
                "{key:?} {modifiers:?}"
            );
        }
    }
    for (key, modifiers) in [
        (Key::Named(NamedKey::Escape), Modifiers::default()),
        (
            Key::Named(NamedKey::Tab),
            Modifiers {
                control: true,
                ..Modifiers::default()
            },
        ),
        (
            Key::Named(NamedKey::Tab),
            Modifiers {
                control: true,
                shift: true,
                ..Modifiers::default()
            },
        ),
    ] {
        assert!(read_only_shortcut(&key, modifiers));
    }
    assert!(!read_only_shortcut(
        &Key::Named(NamedKey::Tab),
        Modifiers::default()
    ));
}

#[test]
fn read_only_focus_retires_text_overlays_and_draws_a_scaled_focus_border() {
    use onestore::page::{PageObject, Unsupported};
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Draft".into(), Default::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    let page = Page {
        identity: None,
        created: None,
        title: String::new(),
        margin_origin: [0.0; 2],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects: vec![PageObject::Unsupported(Unsupported {
            id: Default::default(),
            jcid: 0xdead,
            layout: onestore::document::Layout {
                x: Some(100.0),
                y: Some(80.0),
                max_width: Some(180.0),
                max_height: Some(80.0),
                ..Default::default()
            },
        })],
    };
    let scene = (PageScene::new(page, &mut engine).unwrap(), [20.0, 30.0]);
    let original = editor.active_outline().document().clone();
    let rectangles = |primitives: Vec<Primitive<'_>>| {
        primitives
            .into_iter()
            .filter_map(|p| match p {
                Primitive::Rect { rect, color } | Primitive::RoundedRect { rect, color, .. } => {
                    Some((rect, color))
                }
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    for scale in [1.0, 2.0] {
        let text = rectangles(
            page_primitives(&editor, Some(&scene), None, None, paint(true, scale, 1.0)).unwrap(),
        );
        assert!(text.iter().any(|(_, color)| *color == COLORS.caret));
        let selected = rectangles(
            page_primitives(
                &editor,
                Some(&scene),
                None,
                Some(ObjectFocus::ReadOnly(0)),
                paint(true, scale, 1.0),
            )
            .unwrap(),
        );
        assert!(!selected.iter().any(|(_, color)| *color == COLORS.caret));
        assert_eq!(
            selected,
            rectangles(
                page_primitives(
                    &editor,
                    Some(&scene),
                    None,
                    Some(ObjectFocus::ReadOnly(0)),
                    paint(false, scale, 1.0)
                )
                .unwrap()
            )
        );
        assert_eq!(
            selected.last().unwrap().0,
            [300.0 - 2.0 / scale, 110.0, 300.0, 190.0]
        );
    }
    editor.select_all().unwrap();
    let text = rectangles(
        page_primitives(&editor, Some(&scene), None, None, paint(false, 1.0, 1.0)).unwrap(),
    );
    assert!(text.iter().any(|(_, color)| *color == COLORS.selection));
    let selected = rectangles(
        page_primitives(
            &editor,
            Some(&scene),
            None,
            Some(ObjectFocus::ReadOnly(0)),
            paint(true, 1.0, 1.0),
        )
        .unwrap(),
    );
    assert!(!selected.iter().any(|(_, color)| *color == COLORS.selection));
    assert_eq!(editor.active_outline().document(), &original);
}

/// A view at 100% on a 1x display over one outline holding "Before", a 40 x 30 pt picture and
/// "After" at (36, 36), with the picture's identity.
fn picture_view() -> (PageView, onestore::ExGuid) {
    use onestore::page::{Image, PageObject, ParagraphContent};
    let mut engine = TextEngine::default();
    let mut source = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![
            Paragraph::new("Before".into(), Default::default()),
            Paragraph::new("After".into(), Default::default()),
        ])
        .unwrap(),
        240.0,
        [36.0, 36.0],
    )
    .unwrap()
    .snapshot();
    let mut picture = source.paragraphs[0].clone();
    picture.id = onestore::page::text::new_id().unwrap();
    let id = onestore::page::text::new_id().unwrap();
    picture.content = ParagraphContent::Image(Image {
        size: Some([40.0, 30.0]),
        id,
        layout: Default::default(),
        bytes: None,
        display: None,
        alt: None,
        background: false,
        printout: None,
        tags: Vec::new(),
        link: None,
        text: None,
    });
    source.paragraphs.insert(1, picture);
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![PageObject::Outline(source)],
        },
        &mut engine,
    )
    .unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    (view, id)
}

fn pasted_picture_view() -> PageView {
    let (mut view, _) = picture_view();
    let (scene, editor) =
        PageScene::from_page(view.editor.page().unwrap(), &mut view.engine).unwrap();
    view.editor = editor;
    view.scene = Some((scene, [0.0; 2]));
    let bytes = include_bytes!("../../../../corpus/object-tags/native/notebook/photo.png");
    let _ = view.insert_picture(bytes.to_vec(), [40.0, 30.0]).unwrap();
    let (scene, _) = view.scene.as_mut().unwrap();
    scene.settle(Some(&view.editor), 1.0, COLORS.paper);
    view
}

#[test]
fn a_picture_pasted_into_an_open_outline_reaches_the_scene() {
    let view = pasted_picture_view();
    assert!(view.primitives(COLORS).unwrap().iter().any(|primitive| {
        matches!(primitive, Primitive::Image { rect, .. }
            if (rect[2] - rect[0] - 40.0).abs() < 0.01
                && (rect[3] - rect[1] - 30.0).abs() < 0.01)
    }));
}

#[test]
#[ignore = "requires a native GPU adapter"]
fn a_pasted_inline_picture_is_drawn_on_the_gpu() {
    let view = pasted_picture_view();
    let primitives = view.primitives(COLORS).unwrap();
    let rect = primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::Image { rect, .. } => Some(*rect),
            _ => None,
        })
        .unwrap();
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let size = [512, 256];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Pasted picture"),
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
    let mut renderer = draw::Renderer::new(
        device.clone(),
        queue.clone(),
        wgpu::TextureFormat::Rgba8UnormSrgb,
    );
    renderer
        .draw(
            &texture.create_view(&Default::default()).into(),
            size,
            [1.0; 4],
            &[draw::Layer {
                scale: 1.0,
                origin: [0.0; 2],
                clip: None,
                backdrop: None,
                round: None,
                motion: None,
                primitives: &primitives,
            }],
        )
        .unwrap();
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Pasted picture pixels"),
        size: u64::from(size[0] * size[1] * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size[0] * 4),
                rows_per_image: Some(size[1]),
            },
        },
        texture.size(),
    );
    queue.submit([encoder.finish()]);
    let (sender, receiver) = std::sync::mpsc::channel();
    readback.map_async(wgpu::MapMode::Read, .., move |result| {
        sender.send(result).unwrap();
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: Some(Duration::from_secs(5)),
        })
        .unwrap();
    receiver
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    let pixels = readback.get_mapped_range(..).unwrap();
    let colored = (rect[1].ceil() as usize..rect[3].floor() as usize)
        .flat_map(|y| {
            (rect[0].ceil() as usize..rect[2].floor() as usize)
                .map(move |x| (y * size[0] as usize + x) * 4)
        })
        .filter(|&at| {
            let rgb = &pixels[at..at + 3];
            rgb.iter().max().unwrap() - rgb.iter().min().unwrap() > 30
        })
        .count();
    assert!(
        colored > 100,
        "picture rectangle contained only {colored} colored pixels"
    );
    if let Some(path) = std::env::var_os("SNOWBOUND_PICTURE_CAPTURE") {
        let mut encoder = png::Encoder::new(std::fs::File::create(path).unwrap(), size[0], size[1]);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&pixels)
            .unwrap();
    }
}

#[test]
fn a_picture_context_copies_and_cuts_the_picture_instead_of_hidden_text() {
    use onestore::page::{PageObject, ParagraphContent};
    for floating in [false, true] {
        let (mut view, _) = picture_view();
        let bytes = include_bytes!("../../../../corpus/object-tags/native/notebook/photo.png");
        let id = if floating {
            let _ = view
                .drop_picture([400.0, 200.0], bytes.to_vec(), [40.0, 30.0])
                .unwrap();
            view.editor
                .page()
                .unwrap()
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Image(image) => Some(image.id),
                    _ => None,
                })
                .unwrap()
        } else {
            let image = crate::editor::picture(bytes.to_vec(), [40.0, 30.0]).unwrap();
            let id = image.id;
            view.editor.insert_picture(&mut view.engine, image).unwrap();
            id
        };
        let (scene, _) =
            PageScene::from_page(view.editor.page().unwrap(), &mut view.engine).unwrap();
        view.scene = Some((scene, [0.0; 2]));
        let (origin, size) = view.editor.image_placement(id).unwrap();
        let _ = view
            .pointer_moved([origin[0] + size[0] / 2.0, origin[1] + size[1] / 2.0])
            .unwrap();
        let (_, context) = view.context().unwrap().unwrap();
        assert_eq!(context.image.as_ref().unwrap().id, id);
        assert!(context.selected);
        let copied = view.copy(false).unwrap();
        let Some(Request::Copy(clip)) = copied.request else {
            panic!("a picture copy");
        };
        assert_eq!(clip.paragraphs.len(), 1);
        let ParagraphContent::Image(picture) = &clip.paragraphs[0].content else {
            panic!("a picture");
        };
        assert_eq!(picture.id, id);
        assert_eq!([picture.layout.x, picture.layout.y], [None; 2]);
        assert_eq!(picture.bytes.as_deref(), Some(bytes.as_slice()));
        assert_eq!(clip.text(), "");
        assert!(view.editor.image_placement(id).is_some());
        let cut = view.copy(true).unwrap();
        assert!(cut.changed);
        assert!(matches!(cut.request, Some(Request::Copy(_))));
        assert!(view.editor.image_placement(id).is_none());
        assert!(view.undo(false).unwrap().changed);
        assert!(view.editor.image_placement(id).is_some());
        assert!(view.undo(true).unwrap().changed);
        assert!(view.editor.image_placement(id).is_none());
        view.editor
            .place_caret(&mut view.engine, [600.0, 400.0], 240.0)
            .unwrap();
        assert!(view.paste_clip(clip).unwrap().changed);
        let page = view.editor.page().unwrap();
        assert!(
            page.objects
                .iter()
                .filter_map(|object| match object {
                    PageObject::Outline(outline) => Some(outline),
                    _ => None,
                })
                .flat_map(|outline| &outline.paragraphs)
                .filter_map(|node| node.text())
                .any(|text| text.text.text() == "Before")
        );
        let pasted = view
            .editor
            .outlines()
            .iter()
            .flat_map(|outline| crate::document::descendants(outline.document().nodes(), None))
            .find_map(|(_, _, node)| match &node.content {
                ParagraphContent::Image(image) if image.bytes.is_some() => Some(image.id),
                _ => None,
            })
            .unwrap();
        let (scene, _) = view.scene.as_mut().unwrap();
        scene.settle(Some(&view.editor), 1.0, crate::gpu::Paper::WHITE);
        assert!(scene.image(pasted).is_some());
    }
}

#[test]
fn restoring_a_pictures_original_size_is_undoable_in_an_outline_and_on_the_page() {
    for floating in [false, true] {
        let (mut view, _) = picture_view();
        let bytes = include_bytes!("../../../../corpus/object-tags/native/notebook/photo.png");
        let id = if floating {
            let _ = view
                .drop_picture([400.0, 200.0], bytes.to_vec(), [40.0, 30.0])
                .unwrap();
            view.editor
                .page()
                .unwrap()
                .objects
                .iter()
                .find_map(|object| match object {
                    onestore::page::PageObject::Image(image) => Some(image.id),
                    _ => None,
                })
                .unwrap()
        } else {
            let image = crate::editor::picture(bytes.to_vec(), [40.0, 30.0]).unwrap();
            let id = image.id;
            view.editor.insert_picture(&mut view.engine, image).unwrap();
            id
        };
        let (scene, _) =
            PageScene::from_page(view.editor.page().unwrap(), &mut view.engine).unwrap();
        view.scene = Some((scene, [0.0; 2]));
        let (origin, _) = view.editor.image_placement(id).unwrap();
        view.editor
            .place_image(&mut view.engine, id, origin, [80.0, 60.0])
            .unwrap();
        let _ = view
            .pointer_moved([origin[0] + 10.0, origin[1] + 10.0])
            .unwrap();
        let (_, context) = view.context().unwrap().unwrap();
        assert_eq!(context.image.unwrap().id, id);
        assert!(view.restore_picture_size(id).unwrap().changed);
        assert_eq!(
            view.editor.image_placement(id),
            Some((origin, [40.0, 30.0]))
        );
        assert!(view.undo(false).unwrap().changed);
        assert_eq!(
            view.editor.image_placement(id),
            Some((origin, [80.0, 60.0]))
        );
        assert!(view.undo(true).unwrap().changed);
        assert_eq!(
            view.editor.image_placement(id),
            Some((origin, [40.0, 30.0]))
        );
    }
}

#[test]
fn a_page_outline_emptied_by_backspace_still_draws() {
    let mut engine = TextEngine::default();
    let objects = [[36.0, 36.0], [36.0, 200.0]]
        .map(|origin| {
            let document =
                TextDocument::new(vec![Paragraph::new("Delete me".into(), Default::default())])
                    .unwrap();
            let outline = TextOutline::new(&mut engine, document, 240.0, origin).unwrap();
            onestore::page::PageObject::Outline(outline.snapshot())
        })
        .into();
    let page = Page {
        title: String::new(),
        identity: None,
        created: None,
        margin_origin: [36.0, 14.4],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects,
    };
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let other = editor.outlines()[1].id;
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    view.editor.select_all().unwrap();
    assert!(
        view.key(&Key::Named(NamedKey::Backspace), None)
            .unwrap()
            .changed
    );
    assert!(view.editor.caret_outline().is_some());
    view.primitives(COLORS).unwrap();
    view.editor.focus_outline(other).unwrap();
    assert_eq!(view.editor.outlines().len(), 1);
    view.primitives(COLORS).unwrap();
}

fn click(view: &mut PageView, point: [f32; 2], now: Instant) {
    let _ = view.pointer_moved(point).unwrap();
    assert!(view.pointer_pressed(now).unwrap().changed);
    assert!(view.pointer_released().unwrap().changed);
}

#[test]
fn events_route_through_the_view_as_the_host_delivers_them() {
    let (mut view, picture) = picture_view();
    let now = Instant::now();
    let outline = view.editor.active_outline().id;
    let bounds = view.editor.active_outline().bounds();

    // Typing at a clicked caret edits the outline and asks for nothing else.
    click(
        &mut view,
        [bounds.x0 as f32 + 2.0, bounds.y0 as f32 + 6.0],
        now,
    );
    assert_eq!(view.cursor(), Cursor::Text);
    let typed = view.key(&Key::Character("X".into()), Some("X")).unwrap();
    assert!(typed.changed && typed.request.is_none());
    assert!(
        view.editor
            .active_outline()
            .document()
            .paragraphs()
            .next()
            .unwrap()
            .text()
            .starts_with('X')
    );

    // Copy asks the host to copy; the host's paste commits its text. The shortcut
    // modifier's chords are the host's, so the page takes none of them as typing.
    let _ = view
        .modifiers_changed(Modifiers {
            shift: true,
            ..Modifiers::default()
        })
        .unwrap();
    let _ = view.key(&Key::Named(NamedKey::ArrowRight), None).unwrap();
    assert_eq!(copied(view.copy(false).unwrap()).as_deref(), Some("B"));
    let _ = view
        .modifiers_changed(Modifiers {
            command: true,
            ..Modifiers::default()
        })
        .unwrap();
    assert_eq!(
        view.key(&Key::Character("v".into()), Some("v")).unwrap(),
        Response::default()
    );
    assert!(view.commit_text("line\nbreak".into()).unwrap().changed);
    assert_eq!(
        view.editor.active_outline().document().paragraphs().count(),
        3
    );

    // Select All selects the caret's paragraph, then the whole outline.
    let caret = view.editor.selection().positions[1].paragraph;
    let select_all = |view: &mut PageView| {
        let _ = view.widen_selection().unwrap();
        view.editor
            .selection()
            .positions
            .map(|position| (position.paragraph, position.offset))
    };
    let last = view
        .editor
        .active_outline()
        .document()
        .paragraphs()
        .last()
        .unwrap();
    let whole = [(0, 0), (2, last.utf16_offset(last.text().len()).unwrap())];
    let paragraph = select_all(&mut view);
    assert!(
        paragraph[0] == (caret, 0) && paragraph != whole,
        "{paragraph:?}"
    );
    assert_eq!(select_all(&mut view), whole);
    let _ = view.modifiers_changed(Modifiers::default()).unwrap();

    // A click on the picture selects it, stops text input, and Delete removes it.
    let (origin, size) = view.editor.image_placement(picture).unwrap();
    let center = [origin[0] + size[0] / 2.0, origin[1] + size[1] / 2.0];
    click(&mut view, center, now + Duration::from_secs(1));
    assert_eq!(view.object_focus(), Some(ObjectFocus::Image(picture)));
    assert!(!view.accepts_text());
    assert_eq!(view.cursor(), Cursor::Move);
    assert_eq!(
        view.commit_text("ignored".into()).unwrap(),
        Response::default()
    );
    assert!(
        view.key(&Key::Named(NamedKey::Delete), None)
            .unwrap()
            .changed
    );
    assert!(view.editor.image_placement(picture).is_none());
    assert!(view.accepts_text());

    // The header drags the outline onto the grid anchored at the page margin.
    let bounds = view.editor.active_outline().bounds();
    let header = [(bounds.x0 + bounds.x1) as f32 / 2.0, bounds.y0 as f32 - 8.0];
    let _ = view.pointer_moved(header).unwrap();
    let _ = view.pointer_pressed(now + Duration::from_secs(2)).unwrap();
    assert_eq!(view.cursor(), Cursor::Move);
    let _ = view
        .pointer_moved([header[0] + 30.0, header[1] + 20.0])
        .unwrap();
    assert_eq!(view.outline_preview().map(|(id, _)| id), Some(outline));
    let _ = view.pointer_released().unwrap();
    let origin = view.editor.active_outline().origin();
    assert_eq!(origin, [72.0, 50.4]);

    // Dropped outside the view, as past the window's corner, it stays where it was.
    let bounds = view.editor.active_outline().bounds();
    let header = [(bounds.x0 + bounds.x1) as f32 / 2.0, bounds.y0 as f32 - 8.0];
    let _ = view.pointer_moved(header).unwrap();
    let _ = view.pointer_pressed(now + Duration::from_secs(4)).unwrap();
    let _ = view.pointer_moved([-200.0, -150.0]).unwrap();
    assert!(view.outline_preview().is_some());
    let _ = view.pointer_released().unwrap();
    assert_eq!(view.editor.active_outline().origin(), origin);

    // An empty spot places a new caret there, 7 px above the pointer on the grid.
    click(&mut view, [400.0, 300.0], now + Duration::from_secs(3));
    let caret = view.editor.caret_outline().unwrap().origin();
    assert_eq!(caret, snap_to_grid([400.0, 293.0], [36.0, 14.4]));

    // With Snap to Grid off it lands where the pointer is, as OneNote 2010 places it.
    view.snap_to_grid = false;
    click(&mut view, [410.0, 330.0], now + Duration::from_secs(5));
    let caret = view.editor.caret_outline().unwrap().origin();
    assert_eq!(caret, [410.0, 323.0]);
}

#[test]
fn toolbar_commands_wait_for_composition_and_object_focus() {
    use crate::editor::{Formatting, Toggle};
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let bold = Formatting::Toggle(Toggle::Bold);
    view.editor.select_all().unwrap();
    let _ = view.compose("に".into(), None).unwrap();
    assert_eq!(view.format(bold.clone()).unwrap(), Response::default());
    let _ = view.cancel_composition().unwrap();
    view.editor.select_all().unwrap();
    view.set_object_focus(Some(ObjectFocus::ReadOnly(0)));
    assert_eq!(view.format(bold.clone()).unwrap(), Response::default());
    view.set_object_focus(None);
    assert!(view.format(bold).unwrap().changed);
    assert_eq!(view.editor.format_state().unwrap().toggles, [Toggle::Bold]);
}

#[test]
fn outline_chrome_grows_left_to_contain_its_tag_column() {
    use crate::editor::{Formatting, NoteTag};
    let mut engine = TextEngine::default();
    let outline = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![
            Paragraph::new("Plain one".into(), Format::default()),
            Paragraph::new("Plain two".into(), Format::default()),
        ])
        .unwrap(),
        468.0,
        [288.0, 36.0],
    )
    .unwrap();
    let mut editor =
        CanvasEditor::from_text_outlines(vec![outline], Default::default(), None).unwrap();
    // OneNote 2010 at 100%: the border sits 15 px left of the text, plus 16 px per tag slot,
    // and the text and right border stay put.
    let pixel = 0.75;
    let (plain, _) = outline_chrome(editor.active_outline(), pixel);
    let check = |editor: &CanvasEditor, slots: f32| {
        let outline = editor.active_outline();
        let (frame, _) = outline_chrome(outline, pixel);
        assert_eq!(outline.origin()[0], 288.0);
        assert!(((288.0 - frame[0]) / pixel - (15.0 + 16.0 * slots)).abs() < 0.01);
        assert_eq!(frame[2], plain[2]);
        let shaped = outline.shaped();
        for (_, paragraph) in outline.layouts() {
            for tag in &paragraph.tags {
                let x = outline.origin()[0] + shaped.tag_column_offset() + tag.origin[0];
                let y = outline.origin()[1] + paragraph.origin[1] + tag.origin[1];
                let size = crate::outline::ParagraphTag::SIZE;
                assert!(frame[0] < x && x + size < frame[2], "{frame:?} {x}");
                assert!(frame[1] < y && y + size < frame[3], "{frame:?} {y}");
                // OneNote 2010 shows an arrow over a check box, which a click toggles.
                let hit = page_hit_test(editor, None, [x + 1.0, y + 1.0], pixel);
                if tag.icon.checkable() {
                    assert_eq!(
                        hit,
                        Some(Hit::Check {
                            outline: outline.id,
                            paragraph: paragraph.id,
                        })
                    );
                } else {
                    assert!(matches!(hit, Some(Hit::Text { .. })));
                }
            }
        }
    };
    check(&editor, 0.0);
    editor
        .format(
            &mut engine,
            Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
        )
        .unwrap();
    check(&editor, 1.0);
    editor
        .format(
            &mut engine,
            Formatting::Tag(NoteTag::defaults()[2].clone(), 2),
        )
        .unwrap();
    check(&editor, 2.0);
    editor
        .format(
            &mut engine,
            Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
        )
        .unwrap();
    editor
        .format(
            &mut engine,
            Formatting::Tag(NoteTag::defaults()[2].clone(), 2),
        )
        .unwrap();
    check(&editor, 0.0);
}

#[test]
fn a_click_on_a_check_box_toggles_it_under_an_arrow() {
    use crate::editor::{Formatting, NoteTag};
    let mut engine = TextEngine::default();
    let outline = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Task".into(), Format::default())]).unwrap(),
        468.0,
        [288.0, 36.0],
    )
    .unwrap();
    let mut editor =
        CanvasEditor::from_text_outlines(vec![outline], Default::default(), None).unwrap();
    editor
        .format(
            &mut engine,
            Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
        )
        .unwrap();
    let outline = editor.active_outline();
    let (_, paragraph) = outline.layouts().next().unwrap();
    let tag = &paragraph.tags[0];
    let centre = [
        outline.origin()[0] + outline.shaped().tag_column_offset() + tag.origin[0] + tag.size / 2.0,
        outline.origin()[1] + paragraph.origin[1] + tag.origin[1] + tag.size / 2.0,
    ];
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let device = [0, 1].map(|axis| centre[axis] * view.viewport.scale + view.viewport.origin[axis]);
    let _ = view.pointer_moved(device).unwrap();
    assert_eq!(view.cursor(), Cursor::Default);
    let status = |view: &PageView| {
        view.editor.active_outline().document().nodes()[0]
            .text()
            .unwrap()
            .tags[0]
            .status
    };
    assert!(view.pointer_pressed(Instant::now()).unwrap().changed);
    let _ = view.pointer_released().unwrap();
    assert_eq!(status(&view), 1);
    let _ = view.pointer_moved([device[0] + 40.0, device[1]]).unwrap();
    assert_eq!(view.cursor(), Cursor::Text);
}

/// OneNote 2010 moves an outline by the drag from its stored position and snaps it to the
/// nearest 18 pt step from the page's margin origin, (36, 14.4) until one is stored.
#[test]
fn a_dragged_outline_snaps_to_the_margin_grid_as_onenote_stores_it() {
    let margin = [36.0, 14.4];
    for (stored, drag, expected) in [
        ([72.0, 72.0], [15.0, 0.0], [90.0, 68.4]),
        ([90.0, 68.4], [5.25, 15.0], [90.0, 86.4]),
        ([0.0, 8.0], [30.0, 0.0], [36.0, 14.4]),
    ] {
        let point = [stored[0] + drag[0], stored[1] + drag[1]];
        let snapped = snap_to_grid(point, margin);
        assert!(
            (0..2).all(|axis| (snapped[axis] - expected[axis]).abs() < 1e-4),
            "{snapped:?} {expected:?}"
        );
    }
}

/// OneNote 2010 at 75 to 150% opens a page scrolled fully left and up: 11 px beyond 7.5 pt
/// left of text, a list marker, 0.75 pt into a tag's slot, or 6 pt above an outline, never
/// right of or below the page origin, even when the only outline lies below the view.
#[test]
fn a_page_opens_where_onenote_places_the_view() {
    use crate::editor::{Formatting, NoteTag};
    let open = |origin: [f32; 2], tag: bool| {
        let mut engine = TextEngine::default();
        let outline = TextOutline::new(
            &mut engine,
            TextDocument::new(vec![Paragraph::new("Plain".into(), Format::default())]).unwrap(),
            468.0,
            origin,
        )
        .unwrap();
        let mut editor =
            CanvasEditor::from_text_outlines(vec![outline], Default::default(), None).unwrap();
        if tag {
            editor
                .format(
                    &mut engine,
                    Formatting::Tag(NoteTag::defaults()[0].clone(), 0),
                )
                .unwrap();
        }
        let reached = reach(editor.active_outline());
        let view = PageView::new(
            editor,
            engine,
            None,
            [800, 600],
            1.0,
            Duration::from_millis(500),
        );
        (reached, view.viewport.origin)
    };
    // Native at 100%: page origin 21 px right and 20 px down, 74 and 60 px, 37 px and 0.
    for (origin, tag, reached, placed) in [
        ([0.0, 0.0], false, [-7.5, -6.0], [21.0, 19.0]),
        ([-40.0, -30.0], false, [-47.5, -36.0], [74.33, 59.0]),
        ([0.0, 40.0], true, [-19.5, 34.0], [37.0, 0.0]),
        ([72.0, 72.0], false, [64.5, 66.0], [0.0, 0.0]),
    ] {
        let (actual, view) = open(origin, tag);
        assert_eq!(actual, reached);
        assert!(
            (0..2).all(|axis| (view[axis] - placed[axis]).abs() < 0.01),
            "{origin:?} {view:?}"
        );
    }
    let (_, view) = open([50.0, 900.0], false);
    assert_eq!(view, [0.0, 0.0]);
}

/// OneNote 2010 returns to a page at the scroll, in device pixels whatever the zoom, and the
/// selection it was left with, without revealing the selection.
#[test]
fn a_page_reopens_where_it_was_left() {
    use crate::document::TextPosition;
    let mut engine = TextEngine::default();
    let outlines: Vec<TextOutline> = [("First", [0.0, 0.0]), ("Far below", [50.0, 900.0])]
        .into_iter()
        .map(|(text, origin)| {
            let paragraph = Paragraph::new(text.into(), Format::default());
            let document = TextDocument::new(vec![paragraph]).unwrap();
            TextOutline::new(&mut engine, document, 468.0, origin).unwrap()
        })
        .collect();
    let editor = |count: usize| {
        CanvasEditor::from_text_outlines(outlines[..count].to_vec(), Default::default(), None)
            .unwrap()
    };
    let mut view = PageView::new(
        editor(2),
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let far = outlines[1].id;
    view.editor.focus_outline(far).unwrap();
    let selection = Selection::from([2, 5].map(|offset| TextPosition {
        paragraph: 0,
        offset,
    }));
    view.editor.select(selection).unwrap();
    let _ = view.scroll_to(1, 300.0).unwrap();
    let place = view.place();

    view.open(editor(2), None, None);
    assert_eq!(view.viewport.origin, [21.0, 19.0]);
    let _ = view.set_zoom(2.0).unwrap();
    view.open(editor(2), None, Some(place));
    assert_eq!(view.viewport.origin, [21.0, -300.0]);
    assert_eq!(view.editor.active_outline().id, far);
    assert_eq!(view.editor.selection(), selection);

    // A place past the page's content is kept within it, here 6 pt above the outline at 200%.
    view.open(editor(1), None, Some(place));
    assert_eq!(view.viewport.origin, [21.0, 27.0]);
    assert_eq!(view.editor.active_outline().id, outlines[0].id);
}

#[test]
fn page_keys_scroll_or_carry_the_caret_as_the_platform_does() {
    let mut engine = TextEngine::default();
    let lines = (0..100)
        .map(|line| Paragraph::new(format!("Line {line}"), Default::default()))
        .collect();
    let outline = TextOutline::new(
        &mut engine,
        TextDocument::new(lines).unwrap(),
        240.0,
        [36.0, 36.0],
    )
    .unwrap();
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Outline(outline.snapshot())],
        },
        &mut engine,
    )
    .unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 200],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    let page_down = Key::Named(NamedKey::PageDown);
    let option = Modifiers {
        option: true,
        ..Modifiers::default()
    };
    for modifiers in [Modifiers::default(), option] {
        view.viewport.origin = [0.0; 2];
        view.editor
            .move_selection(&mut view.engine, Movement::DocumentStart, false)
            .unwrap();
        let _ = view.modifiers_changed(modifiers).unwrap();
        let caret = view.caret_area().unwrap()[1];
        let _ = view.key(&page_down, None).unwrap();
        let scrolled = -view.viewport.origin[1];
        let shown = view.caret_area().unwrap()[1];
        match Command::from_key(&page_down, modifiers) {
            Some(Command::ScrollPage { up: false }) => {
                assert_eq!(scrolled, 190.0, "a page less AppKit's ten points");
                assert_eq!(shown, caret - scrolled, "the caret stays in the text");
            }
            Some(Command::MovePage { up: false }) => {
                assert!(scrolled >= 200.0, "{scrolled}");
                assert_eq!(shown, caret, "the caret keeps its place on screen");
            }
            None => assert_eq!((scrolled, shown), (0.0, caret)),
            other => panic!("Page Down is {other:?}"),
        }
    }
}

#[test]
fn an_empty_preedit_without_a_composition_changes_nothing() {
    let (mut view, _) = picture_view();
    assert_eq!(
        view.compose(String::new(), None).unwrap(),
        Response::default()
    );
    assert!(view.compose("k".into(), Some((1, 1))).unwrap().changed);
    assert!(view.compose(String::new(), None).unwrap().changed);
    assert!(view.editor.marked_range().is_none());
}

/// The lab's page: a title, then outlines placed in the order Alpha, Beta, Gamma, Delta, with
/// Delta highest on the page, drawn at 1 point per pixel.
fn page_selection_view() -> (PageView, [onestore::ExGuid; 4]) {
    let mut engine = TextEngine::default();
    let outline = |engine: &mut TextEngine, text: &str, origin| {
        let paragraphs = text
            .split('\n')
            .map(|line| Paragraph::new(line.into(), Default::default()))
            .collect();
        TextOutline::new(
            engine,
            TextDocument::new(paragraphs).unwrap(),
            240.0,
            origin,
        )
        .unwrap()
        .snapshot()
    };
    let mut title = outline(&mut engine, "Multi", [0.0; 2]);
    title.title = true;
    let body = [
        ("Alpha one\nAlpha two", [36.0, 90.0]),
        ("Beta", [300.0, 180.0]),
        ("Gamma long line of text", [90.0, 300.0]),
        ("Delta", [380.0, 72.0]),
    ]
    .map(|(text, origin)| outline(&mut engine, text, origin));
    let ids = body.each_ref().map(|outline| outline.id);
    let page = Page {
        title: "Multi".into(),
        identity: None,
        created: None,
        margin_origin: [36.0, 14.4],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects: std::iter::once(onestore::page::PageObject::Title(onestore::page::Title {
            id: onestore::page::text::new_id().unwrap(),
            date: None,
            layout: Default::default(),
            outlines: vec![title],
        }))
        .chain(body.map(onestore::page::PageObject::Outline))
        .collect(),
    };
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    (view, ids)
}

const START: TextPosition = TextPosition {
    paragraph: 0,
    offset: 0,
};

fn select_page(view: &mut PageView, from: onestore::ExGuid) {
    view.editor.focus_outline(from).unwrap();
    view.editor.select([START; 2].into()).unwrap();
    while view.editor.whole() != Some(Whole::Page) {
        let _ = view.widen_selection().unwrap();
    }
}

fn origins(view: &PageView) -> Vec<[f32; 2]> {
    let body = view
        .editor
        .outlines()
        .iter()
        .filter(|outline| !outline.title);
    body.map(TextOutline::origin).collect()
}

/// As OneNote 2010 widens Ctrl+A past an outline (lab, 2026-09-28): a body outline's
/// paragraph, the outline as an object, then every body outline, focusing the last.
#[test]
fn select_all_widens_from_an_outline_to_the_page() {
    let (mut view, [alpha, beta, _, delta]) = page_selection_view();
    let levels = |view: &mut PageView, from| {
        view.editor.focus_outline(from).unwrap();
        view.editor.select([START; 2].into()).unwrap();
        (0..4)
            .map(|_| {
                let _ = view.widen_selection().unwrap();
                (view.editor.whole(), view.editor.active_outline().id)
            })
            .collect::<Vec<_>>()
    };
    let page = (Some(Whole::Page), delta);
    for from in [alpha, beta] {
        assert_eq!(
            levels(&mut view, from),
            [(None, from), (Some(Whole::Outline), from), page, page]
        );
    }
    // The title has no object level: its text, then the page.
    let title = view.editor.outlines()[0].id;
    assert_eq!(levels(&mut view, title), [(None, title), page, page, page]);
    // The selection it chose ends with any other; an arrow leaves the caret at its start.
    let _ = view.key(&Key::Named(NamedKey::ArrowLeft), None).unwrap();
    assert_eq!(view.editor.whole(), None);
    assert_eq!(view.editor.active_outline().id, delta);
    assert_eq!(view.editor.selection().positions, [START; 2]);
}

/// Each outline Select All holds shows its frame and a grey body, its text highlighted.
#[test]
fn a_page_selection_frames_and_greys_every_body_outline() {
    let (mut view, [alpha, ..]) = page_selection_view();
    let grey = crate::gpu::colorref(0x00f0f0f0);
    let paint = |view: &PageView| {
        let primitives = view.primitives(COLORS).unwrap();
        let count =
            |wanted: &dyn Fn(&Primitive) -> bool| primitives.iter().filter(|p| wanted(p)).count();
        (
            count(
                &|p| matches!(p, Primitive::RoundedRect { color, stroke: None, .. } if *color == grey),
            ),
            count(&|p| matches!(p, Primitive::Rect { color, .. } if *color == COLORS.selection)),
        )
    };
    view.editor.focus_outline(alpha).unwrap();
    assert_eq!(paint(&view).0, 0);
    let _ = view.widen_selection().unwrap();
    let _ = view.widen_selection().unwrap();
    let (greyed, alpha) = paint(&view);
    assert_eq!(greyed, 1);
    let _ = view.widen_selection().unwrap();
    // One line each of the others besides Alpha's.
    assert_eq!(paint(&view), (4, alpha + 3));
}

/// Keys on the page selection (lab): arrows up and down and Tab leave it, Enter and Right
/// put the caret at the last outline's end, Delete and Backspace remove every outline and
/// leave the caret where the first began, and typing replaces them all with the last outline
/// holding the text. Each edit stores as one edit and undoes in one step to the selection.
#[test]
fn keys_act_on_the_page_selection_as_onenote_does() {
    let (mut view, [alpha, beta, gamma, delta]) = page_selection_view();
    let end = TextPosition {
        paragraph: 0,
        offset: 5,
    };
    for key in [
        NamedKey::ArrowUp,
        NamedKey::ArrowDown,
        NamedKey::Tab,
        NamedKey::Enter,
    ] {
        select_page(&mut view, beta);
        let _ = view.key(&Key::Named(key), None).unwrap();
        let kept = key != NamedKey::Enter;
        assert_eq!(view.editor.whole() == Some(Whole::Page), kept, "{key:?}");
        if !kept {
            assert_eq!(view.editor.selection().positions, [end; 2]);
        }
    }
    assert_eq!(view.editor.take_ops().unwrap(), []);

    let stored = origins(&view);
    for key in [NamedKey::Delete, NamedKey::Backspace] {
        select_page(&mut view, gamma);
        let _ = view.key(&Key::Named(key), None).unwrap();
        assert!(view.editor.outlines().iter().all(|outline| outline.title));
        assert_eq!(
            view.editor.caret_outline().map(TextOutline::origin),
            Some(stored[0])
        );
        let ops = view.editor.take_ops().unwrap();
        let deleted = ops.iter().filter_map(|op| match op {
            onestore::op::PageOp::Delete { object } => Some(*object),
            _ => None,
        });
        assert_eq!(
            deleted.collect::<BTreeSet<_>>(),
            BTreeSet::from([alpha, beta, gamma, delta])
        );
        assert_eq!(ops.len(), 4);
        let _ = view.undo(false).unwrap();
        assert_eq!(origins(&view), stored);
        assert_eq!(view.editor.whole(), Some(Whole::Page));
        view.editor.take_ops().unwrap();
    }

    select_page(&mut view, alpha);
    let _ = view.key(&Key::Character("Z".into()), Some("Z")).unwrap();
    let left = view
        .editor
        .outlines()
        .iter()
        .filter(|outline| !outline.title);
    assert_eq!(
        left.map(|outline| (outline.id, outline.shown_text()))
            .collect::<Vec<_>>(),
        [(delta, "Z".to_owned())]
    );
    let _ = view.undo(false).unwrap();
    assert_eq!(origins(&view), stored);
    assert_eq!(view.editor.whole(), Some(Whole::Page));
    assert_eq!(view.editor.active_outline().shown_text(), "Delta");
}

/// Copy takes the outlines top to bottom with a blank line between, as OneNote's plain text
/// does; Cut removes them as Delete does.
#[test]
fn copy_and_cut_take_the_page_selection() {
    let (mut view, [alpha, ..]) = page_selection_view();
    select_page(&mut view, alpha);
    let text = "Delta\n\nAlpha one\nAlpha two\n\nBeta\n\nGamma long line of text";
    assert_eq!(copied(view.copy(false).unwrap()).as_deref(), Some(text));
    assert_eq!(view.editor.whole(), Some(Whole::Page));
    assert_eq!(copied(view.copy(true).unwrap()).as_deref(), Some(text));
    assert!(view.editor.outlines().iter().all(|outline| outline.title));
    assert!(view.editor.caret_outline().is_some());
}

/// Dragging any selected outline's handle moves them all by one snapped offset, stored as one
/// edit and undone in one step; a click on a handle selects that outline alone.
#[test]
fn a_dragged_page_selection_moves_every_outline() {
    let (mut view, [alpha, beta, ..]) = page_selection_view();
    let stored = origins(&view);
    select_page(&mut view, alpha);
    let now = Instant::now();
    let bounds = view
        .editor
        .outlines()
        .iter()
        .find(|outline| outline.id == beta)
        .unwrap()
        .bounds();
    let header = [(bounds.x0 + bounds.x1) as f32 / 2.0, bounds.y0 as f32 - 8.0];
    let _ = view.pointer_moved(header).unwrap();
    let _ = view.pointer_pressed(now).unwrap();
    assert_eq!(view.editor.whole(), Some(Whole::Page));
    let _ = view
        .pointer_moved([header[0] + 30.0, header[1] + 40.0])
        .unwrap();
    // Every selected outline follows the drag while it lasts.
    let dragged = view.primitives(COLORS).unwrap().len();
    assert!(dragged > 0);
    let _ = view.pointer_released().unwrap();
    // Beta lands on the grid, 18 pt cells from the margin origin, and the rest follow.
    let moved = origins(&view);
    assert_eq!(moved[1], [324.0, 212.4]);
    for (before, after) in stored.iter().zip(&moved) {
        for axis in 0..2 {
            let delta = moved[1][axis] - stored[1][axis];
            assert!((after[axis] - before[axis] - delta).abs() < 1e-3);
        }
    }
    assert_eq!(view.editor.whole(), Some(Whole::Page));
    assert_eq!(view.editor.take_ops().unwrap().len(), 4);
    let _ = view.undo(false).unwrap();
    assert_eq!(origins(&view), stored);

    // Option-Command-arrows nudge the whole selection.
    let _ = view
        .modifiers_changed(Modifiers {
            command: true,
            option: true,
            ..Modifiers::default()
        })
        .unwrap();
    let _ = view.key(&Key::Named(NamedKey::ArrowRight), None).unwrap();
    let _ = view.modifiers_changed(Modifiers::default()).unwrap();
    assert!(
        origins(&view)
            .iter()
            .zip(&stored)
            .all(|(after, before)| *after == [before[0] + 1.0, before[1]])
    );

    let _ = view.pointer_moved(header).unwrap();
    let _ = view.pointer_pressed(now + Duration::from_secs(1)).unwrap();
    let _ = view.pointer_released().unwrap();
    assert_eq!(view.editor.whole(), Some(Whole::Outline));
    assert_eq!(view.editor.active_outline().id, beta);
}

/// A toggle on the page selection turns on everywhere unless every outline has it, as one
/// undo step that keeps the selection.
#[test]
fn formatting_the_page_selection_settles_on_one_state() {
    let (mut view, [alpha, beta, ..]) = page_selection_view();
    let bold = |view: &mut PageView| {
        let ids = view
            .editor
            .outlines()
            .iter()
            .filter(|outline| !outline.title)
            .map(|outline| outline.id)
            .collect::<Vec<_>>();
        let focus = view.editor.active_outline().id;
        let selection = view.editor.selection();
        let whole = view.editor.whole();
        let states = ids
            .into_iter()
            .map(|id| {
                view.editor.focus_outline(id).unwrap();
                view.editor.select_all().unwrap();
                view.editor
                    .format_state()
                    .unwrap()
                    .toggles
                    .contains(&Toggle::Bold)
            })
            .collect::<Vec<_>>();
        if whole == Some(Whole::Page) {
            select_page(view, focus);
        } else {
            view.editor.focus_outline(focus).unwrap();
            view.editor.select(selection).unwrap();
        }
        states
    };
    view.editor.focus_outline(beta).unwrap();
    view.editor.select_all().unwrap();
    let _ = view.format(Formatting::Toggle(Toggle::Bold)).unwrap();
    assert_eq!(bold(&mut view), [false, true, false, false]);
    select_page(&mut view, alpha);
    let _ = view.format(Formatting::Toggle(Toggle::Bold)).unwrap();
    assert_eq!(view.editor.whole(), Some(Whole::Page));
    assert_eq!(bold(&mut view), [true; 4]);
    let _ = view.format(Formatting::Toggle(Toggle::Bold)).unwrap();
    assert_eq!(bold(&mut view), [false; 4]);
    let _ = view.undo(false).unwrap();
    assert_eq!(bold(&mut view), [true; 4]);
    assert_eq!(view.editor.whole(), Some(Whole::Page));
}

/// Selecting the last outline's text again by other means selects only that text.
#[test]
fn the_page_selection_does_not_return_with_its_text_selection() {
    let (mut view, [alpha, ..]) = page_selection_view();
    select_page(&mut view, alpha);
    let whole = view.editor.selection();
    let _ = view.key(&Key::Named(NamedKey::ArrowRight), None).unwrap();
    view.editor.select(whole).unwrap();
    assert_eq!(view.editor.whole(), None);
}

/// The point under the pointer stays put through a pinch, as in Safari, even where the
/// page's content is too small to scroll that far; scrolling afterwards still reaches it.
#[test]
fn pinch_zooms_about_the_pointer_within_range() {
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("text".into(), Format::default())]).unwrap(),
        240.0,
    )
    .unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let pointer = [700.0, 500.0];
    let under = view.viewport.document_point(pointer);
    let shown = view.viewport.document_point([800.0, 600.0]);
    let stays = |view: &PageView| {
        let after = view.viewport.document_point(pointer);
        assert!(
            (after[0] - under[0]).abs() < 1e-3 && (after[1] - under[1]).abs() < 1e-3,
            "{under:?} moved to {after:?}"
        );
    };
    let zoom = view.zoom();
    for factor in [1.05, 1.0 / 1.05, 1.05] {
        for _ in 0..10 {
            assert!(view.pinch(factor, pointer).unwrap().moved);
            stays(&view);
        }
    }
    assert!((view.zoom() / zoom - 1.05_f32.powi(10)).abs() < 1e-4);
    let _ = view.wheel([-5000.0; 2]).unwrap();
    let corner = view.viewport.document_point([800.0, 600.0]);
    assert!((corner[0] - shown[0]).abs() < 1e-3 && (corner[1] - shown[1]).abs() < 1e-3);
    let _ = view.pinch(100.0, pointer).unwrap();
    assert!((view.zoom() - 4.0).abs() < 1e-5);
}

fn texts(view: &PageView) -> Vec<(String, [f32; 2])> {
    view.editor
        .outlines()
        .iter()
        .filter(|outline| !outline.title)
        .map(|outline| (outline.shown_text(), outline.origin()))
        .collect()
}

/// Drags Insert Space from `from` to `to`, view points at 1:1.
fn insert_space(view: &mut PageView, from: [f32; 2], to: [f32; 2]) {
    let _ = view.insert_space();
    let _ = view.pointer_moved(from).unwrap();
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_moved(to).unwrap();
    let _ = view.pointer_released().unwrap();
}

/// As OneNote 2010's Insert Space (lab, 2026-09-29): what starts at or below the line moves
/// by the drag, an outline it crosses parts between paragraphs into a new outline below, the
/// title stays, and one undo step puts everything back.
#[test]
fn insert_space_moves_what_lies_below_and_parts_a_crossed_outline() {
    let (mut view, [alpha, ..]) = page_selection_view();
    let stored = texts(&view);
    let title = view.editor.outlines()[0].origin();
    let alpha_outline = view
        .editor
        .outlines()
        .iter()
        .find(|o| o.id == alpha)
        .unwrap();
    let second = alpha_outline.origin()[1] + alpha_outline.shaped().paragraphs[1].origin[1];
    let _ = view.insert_space();
    let _ = view.pointer_moved([400.0, second]).unwrap();
    assert!(view.inserting_space());
    assert_eq!(view.cursor(), Cursor::RowResize);
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_moved([400.0, second + 50.0]).unwrap();
    // Nothing moves until the drag ends; its lines, arrow and landing tints show.
    assert_eq!(texts(&view), stored);
    assert!(view.primitives(COLORS).is_ok());
    let _ = view.pointer_released().unwrap();
    assert!(!view.inserting_space());
    let moved = texts(&view);
    let expected: Vec<(String, [f32; 2])> = vec![
        ("Alpha one".into(), stored[0].1),
        ("Alpha two".into(), [stored[0].1[0], second + 50.0]),
        ("Beta".into(), [300.0, 230.0]),
        ("Gamma long line of text".into(), [90.0, 350.0]),
        ("Delta".into(), [380.0, 72.0]),
    ];
    assert_eq!(moved, expected);
    assert_eq!(view.editor.outlines()[0].origin(), title);
    assert!(!view.editor.take_ops().unwrap().is_empty());
    let _ = view.undo(false).unwrap();
    assert_eq!(texts(&view), stored);
    assert_eq!(view.editor.whole(), None);
    let _ = view.undo(true).unwrap();
    assert_eq!(texts(&view), expected);
}

/// Within half an inch of the view's side the line runs down the page and moves what starts
/// right of it; taking space back stops at the end of what stays before the line.
#[test]
fn insert_space_runs_down_the_page_near_its_side_and_takes_back_only_space() {
    let (mut view, _) = page_selection_view();
    let stored = texts(&view);
    let _ = view.insert_space();
    let _ = view.pointer_moved([20.0, 200.0]).unwrap();
    assert_eq!(view.cursor(), Cursor::ColResize);
    let _ = view.pointer_moved([790.0, 200.0]).unwrap();
    assert_eq!(view.cursor(), Cursor::ColResize);
    insert_space(&mut view, [20.0, 200.0], [60.0, 200.0]);
    let right: Vec<_> = stored
        .iter()
        .map(|(text, [x, y])| (text.clone(), [x + 40.0, *y]))
        .collect();
    assert_eq!(texts(&view), right);
    let _ = view.undo(false).unwrap();
    // Undo scrolls to what it restores; the drags below are in page points.
    view.viewport.origin = [0.0; 2];

    // Beta ends at its line's bottom; Gamma, below the line at 250, comes up to it and no
    // further.
    let beta = view.editor.outlines()[2].bounds().y1 as f32;
    insert_space(&mut view, [400.0, 250.0], [400.0, 20.0]);
    let gamma = texts(&view)[2].1[1];
    assert!((gamma - (300.0 - (250.0 - beta))).abs() < 1e-3, "{gamma}");
    // A line with something stationary across it cannot take space back.
    let _ = view.undo(false).unwrap();
    view.viewport.origin = [0.0; 2];
    insert_space(&mut view, [400.0, 185.0], [400.0, 20.0]);
    assert_eq!(texts(&view), stored);
}

#[test]
fn escape_cancels_insert_space() {
    let (mut view, _) = page_selection_view();
    let stored = texts(&view);
    let _ = view.editor.take_ops().unwrap();
    let _ = view.insert_space();
    let _ = view.pointer_moved([400.0, 150.0]).unwrap();
    let _ = view.key(&Key::Named(NamedKey::Escape), None).unwrap();
    assert!(!view.inserting_space());
    let _ = view.insert_space();
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_moved([400.0, 250.0]).unwrap();
    let _ = view.key(&Key::Named(NamedKey::Escape), None).unwrap();
    let _ = view.pointer_released().unwrap();
    assert!(!view.inserting_space());
    assert_eq!(texts(&view), stored);
    assert!(view.editor.take_ops().unwrap().is_empty());
}

/// Rule lines draw as OneNote 2010 draws them: horizontal lines from the margin origin
/// down, a margin line 1/96 inch left of it, and grid lines through it either way.
#[test]
fn rule_lines_start_at_the_margin_origin() {
    // Each line's axis and the centre across it, to a hundredth of a point.
    let lines = |lines| {
        super::rule_primitives(
            Some(lines),
            [36.0, 14.4],
            [0.0, 0.0, 100.0, 100.0],
            0.5,
            Paper::WHITE,
        )
        .into_iter()
        .map(|primitive| match primitive {
            Primitive::Rect {
                rect: [x0, y0, x1, y1],
                ..
            } => {
                let vertical = y1 - y0 > x1 - x0;
                let center = if vertical { x0 + x1 } else { y0 + y1 } / 2.0;
                (vertical, (center * 100.0).round() / 100.0)
            }
            _ => panic!("rule lines are rectangles"),
        })
        .collect::<Vec<_>>()
    };
    let [(_, standard), (_, small)] = [
        crate::template::RULE_LINES[2],
        crate::template::RULE_LINES[4],
    ];
    assert_eq!(
        lines(standard),
        [(false, 14.4), (false, 47.52), (false, 80.64), (true, 35.25)]
    );
    let vertical: Vec<f32> = lines(small)
        .into_iter()
        .filter_map(|(vertical, at)| vertical.then_some(at))
        .collect();
    assert_eq!(
        vertical,
        [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0, 96.0]
    );
}

/// Template art the Page Color menu gives draws in the frame after, over the rule lines as
/// OneNote's opaque pictures hide them, and follows undo and redo without a reload.
#[test]
fn art_draws_at_once_over_the_rule_lines_and_follows_undo() {
    let mut engine = TextEngine::default();
    let outline = TextOutline::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new("Notes".into(), Format::default())]).unwrap(),
        300.0,
        [300.0, 100.0],
    )
    .unwrap()
    .snapshot();
    let page = Page {
        identity: None,
        title: String::new(),
        created: None,
        margin_origin: [36.0, 14.4],
        rtl: false,
        color: None,
        rule_lines: Some(crate::template::RULE_LINES[2].1),
        definitions: Default::default(),
        objects: vec![onestore::page::PageObject::Outline(outline)],
    };
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let ivy = crate::template::find("Ivy").unwrap();
    let art = ivy.pictures().unwrap();
    let [x, y] = ivy.art[0].position;
    let [width, height] = ivy.art[0].size.unwrap();
    let drawn = |view: &mut PageView, paper: Paper| {
        view.update_pictures(paper, std::task::Waker::noop());
        let primitives = view.primitives(TextColors { paper, ..COLORS }).unwrap();
        let backing = primitives.iter().position(|primitive| {
            matches!(primitive, Primitive::Rect { rect, color }
                if *rect == [x, y, x + width, y + height] && *color == paper.color)
        });
        let image = primitives
            .iter()
            .position(|primitive| matches!(primitive, Primitive::Image { .. }));
        let lines = primitives.iter().rposition(
            |primitive| matches!(primitive, Primitive::Rect { rect, .. } if rect[0] < x),
        );
        (backing, image, lines)
    };
    let _ = view
        .set_paper(None, view.editor.rule_lines(), Some(art))
        .unwrap();
    let (backing, image, lines) = drawn(&mut view, Paper::WHITE);
    assert!(
        lines < backing && backing < image,
        "{lines:?} {backing:?} {image:?}"
    );
    let dark = Paper {
        color: [0.0137, 0.0144, 0.0159, 1.0],
        ink: [0.791, 0.791, 0.791, 1.0],
    };
    // Dark paper hides the lines in its own colour while its recolouring is made.
    assert!(matches!(drawn(&mut view, dark), (Some(_), None, _)));
    let _ = view.undo(false).unwrap();
    assert!(matches!(drawn(&mut view, Paper::WHITE), (None, None, _)));
    let _ = view.undo(true).unwrap();
    assert!(matches!(
        drawn(&mut view, Paper::WHITE),
        (Some(_), Some(_), _)
    ));
}

#[test]
fn an_attached_file_selects_opens_on_a_double_click_and_deletes() {
    let (mut view, _) = picture_view();
    let now = Instant::now();
    let file = onestore::page::Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "notes.txt".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(Arc::from(b"notes".as_slice())),
        preview: None,
        recording: None,
        tags: Vec::new(),
    };
    view.editor
        .move_selection(&mut view.engine, Movement::DocumentEnd, false)
        .unwrap();
    assert!(view.insert_attachment(file.clone()).unwrap().changed);
    let stored = view.editor.attachment(file.id).unwrap().clone();
    // OneNote draws a broken picture for a file stored without its icon.
    assert!(stored.preview.as_ref().unwrap().starts_with(b"\x89PNG"));
    view.primitives(COLORS).unwrap();

    let [x0, y0, x1, y1] = view.editor.attachment_rect(file.id).unwrap();
    let center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    click(&mut view, center, now);
    assert_eq!(view.object_focus(), Some(ObjectFocus::File(file.id)));
    assert!(!view.accepts_text());
    view.primitives(COLORS).unwrap();
    let _ = view.pointer_moved(center).unwrap();
    assert_eq!(
        view.pointer_pressed(now + Duration::from_millis(100))
            .unwrap()
            .request,
        Some(Request::OpenAttachment(stored.clone()))
    );
    let _ = view.pointer_released().unwrap();
    let (_, context) = view.context().unwrap().unwrap();
    assert_eq!(context.attachment, Some(stored));

    assert!(
        view.key(&Key::Named(NamedKey::Delete), None)
            .unwrap()
            .changed
    );
    assert!(view.editor.attachment(file.id).is_none());
    assert!(view.accepts_text());
}

/// OneNote 2010 puts a file attached or dropped on blank page on the page there, where it
/// selects, drags on the grid, opens and deletes as one (`corpus/attachment-floating`).
#[test]
fn a_file_dropped_on_blank_page_lies_on_the_page_and_drags() {
    use onestore::op::PageOp;
    let mut engine = TextEngine::default();
    let document =
        TextDocument::new(vec![Paragraph::new("Text".into(), Default::default())]).unwrap();
    let outline = TextOutline::new(&mut engine, document, 240.0, [36.0, 36.0]).unwrap();
    let page = Page {
        title: String::new(),
        identity: None,
        created: None,
        margin_origin: [36.0, 14.4],
        rtl: false,
        color: None,
        rule_lines: None,
        definitions: Default::default(),
        objects: vec![onestore::page::PageObject::Outline(outline.snapshot())],
    };
    let (scene, editor) = PageScene::from_page(page, &mut engine).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    let _ = view.editor.take_ops().unwrap();
    let file = onestore::page::Attachment {
        id: onestore::page::text::new_id().unwrap(),
        filename: "float.txt".into(),
        source_path: None,
        size: Some([24.0, 24.0]),
        layout: Default::default(),
        bytes: Some(Arc::from(b"float".as_slice())),
        preview: None,
        recording: None,
        tags: Vec::new(),
    };
    let _ = view.drop_attachment([400.0, 400.0], file.clone()).unwrap();
    assert_eq!(view.editor.outlines().len(), 1);
    let placed = |view: &PageView| {
        view.editor
            .page()
            .unwrap()
            .objects
            .into_iter()
            .find_map(|object| match object {
                onestore::page::PageObject::Attachment(file) => Some(file),
                _ => None,
            })
    };
    // At the caret a click there places, on the 18 pt grid from the margin origin.
    let stored = placed(&view).unwrap();
    assert_eq!(
        [stored.layout.x, stored.layout.y],
        [Some(396.0), Some(392.4)]
    );
    assert_eq!(stored.layout.max_width, Some(54.0));
    assert!(matches!(
        &view.editor.take_ops().unwrap()[..],
        [PageOp::Add { object: onestore::page::PageObject::Attachment(added), before: None }]
            if added.id == file.id
    ));
    assert!(
        view.primitives(COLORS)
            .unwrap()
            .iter()
            .any(|primitive| matches!(primitive, Primitive::Text { .. }))
    );

    let [x0, y0, x1, y1] = view.editor.attachment_rect(file.id).unwrap();
    let center = [(x0 + x1) / 2.0, (y0 + y1) / 2.0];
    let now = Instant::now();
    let _ = view.pointer_moved(center).unwrap();
    let _ = view.pointer_pressed(now).unwrap();
    assert_eq!(view.object_focus(), Some(ObjectFocus::File(file.id)));
    let _ = view
        .pointer_moved([center[0] + 100.0, center[1] + 50.0])
        .unwrap();
    view.primitives(COLORS).unwrap();
    let _ = view.pointer_released().unwrap();
    let moved = placed(&view).unwrap();
    assert_eq!([moved.layout.x, moved.layout.y], [Some(504.0), Some(446.4)]);
    assert!(matches!(
        &view.editor.take_ops().unwrap()[..],
        [PageOp::Outline { object, .. }] if *object == file.id
    ));
    let (_, context) = view.context().unwrap().unwrap();
    assert_eq!(context.attachment.map(|found| found.id), Some(file.id));

    let _ = view.undo(false).unwrap();
    assert_eq!(placed(&view).unwrap().layout.x, Some(396.0));
    let _ = view.undo(true).unwrap();
    assert_eq!(placed(&view).unwrap().layout.x, Some(504.0));
    let _ = view.editor.take_ops().unwrap();

    let [x0, y0, x1, y1] = view.editor.attachment_rect(file.id).unwrap();
    click(
        &mut view,
        [(x0 + x1) / 2.0, (y0 + y1) / 2.0],
        now + Duration::from_secs(1),
    );
    let _ = view.key(&Key::Named(NamedKey::Delete), None).unwrap();
    assert!(placed(&view).is_none());
    assert!(matches!(
        &view.editor.take_ops().unwrap()[..],
        [PageOp::Delete { object }] if *object == file.id
    ));
    let _ = view.undo(false).unwrap();
    assert_eq!(placed(&view).unwrap().layout.x, Some(504.0));
    view.primitives(COLORS).unwrap();
}

/// Words the fake dictionary misses are marked red under their text once checked, except
/// the word still being typed; the context menu offers the dictionary's corrections, and
/// taking one is one edit and one undo step.
#[test]
fn misspelled_words_are_marked_and_corrected_in_one_edit() {
    struct Wake(std::sync::mpsc::Sender<()>);
    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            let _ = self.0.send(());
        }
    }
    let mut engine = TextEngine::default();
    let document = TextDocument::new(vec![Paragraph::new(
        "Ths sentence has the typos".into(),
        Default::default(),
    )])
    .unwrap();
    let outline = TextOutline::new(&mut engine, document, 400.0, [36.0, 36.0]).unwrap();
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Outline(outline.snapshot())],
        },
        &mut engine,
    )
    .unwrap();
    let mut view = PageView::new(editor, engine, None, [800, 600], 1.0, Duration::ZERO);
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    let (woken, wakes) = std::sync::mpsc::channel();
    view.spelling = Some(crate::spelling::Spelling::new(
        Box::new(crate::spelling::tests::Fake),
        Arc::new(Wake(woken)).into(),
    ));
    let red = draw::srgb(0xff, 0x00, 0x00);
    let marks = |view: &PageView| -> Vec<[f32; 2]> {
        view.primitives(COLORS)
            .unwrap()
            .into_iter()
            .filter_map(|primitive| match primitive {
                Primitive::Rect { rect, color } if color == red => Some([rect[0], rect[1]]),
                _ => None,
            })
            .collect()
    };
    assert!(marks(&view).is_empty());
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the spelling thread wakes the host");
    let outline = view.editor.active_outline();
    let [origin_x, origin_y] = outline.origin();
    let [left, right, baseline] = outline.underlines(0, 0..3).unwrap()[0];
    let under = marks(&view);
    // OneNote's pixels along "Ths": a column each, stepping mid, low, mid, high on the
    // three pixel rows from one to three below the baseline.
    let [start, end, line] =
        [origin_x + left, origin_x + right, origin_y + baseline].map(f32::round);
    assert_eq!(under.len(), (end - start) as usize);
    for (column, [x, y]) in under.into_iter().enumerate() {
        assert_eq!(x, start + column as f32);
        assert_eq!(y, line + [2.0, 3.0, 2.0, 1.0][column % 4]);
    }

    let _ = view
        .pointer_moved([origin_x + (left + right) / 2.0, origin_y + baseline - 3.0])
        .unwrap();
    let (_, context) = view.context().unwrap().unwrap();
    let correction = context.spelling.unwrap();
    assert_eq!(correction.word, "Ths");
    assert_eq!(correction.suggestions, ["This", "Thus"]);
    view.editor.take_ops().unwrap();
    assert!(view.correct(&correction, "This").unwrap().changed);
    assert_eq!(
        view.editor.active_outline().shown_text(),
        "This sentence has the typos"
    );
    assert!(!view.editor.take_ops().unwrap().is_empty());
    marks(&view);
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the spelling thread wakes the host");
    assert!(marks(&view).is_empty());
    let _ = view.undo(false).unwrap();
    assert_eq!(
        view.editor.active_outline().shown_text(),
        "Ths sentence has the typos"
    );

    view.editor
        .move_selection(&mut view.engine, Movement::DocumentEnd, false)
        .unwrap();
    let _ = view.key(&Key::Character(" ".into()), Some(" ")).unwrap();
    for letter in ["q", "w", "r", "t"] {
        let _ = view
            .key(&Key::Character(letter.into()), Some(letter))
            .unwrap();
    }
    marks(&view);
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the spelling thread wakes the host");
    let typing = marks(&view).len();
    let _ = view.key(&Key::Character(" ".into()), Some(" ")).unwrap();
    marks(&view);
    wakes
        .recv_timeout(Duration::from_secs(10))
        .expect("the spelling thread wakes the host");
    assert!(marks(&view).len() > typing);
}

/// See Playback scrolls the note playing into view, as OneNote 2010 does
/// (`corpus/recording/native/read/onenote-see-playback-scrolls.png`); the caret stays.
#[test]
fn the_note_playing_scrolls_into_view() {
    let mut engine = TextEngine::default();
    let lines = (0..100)
        .map(|line| Paragraph::new(format!("Line {line}"), Default::default()))
        .collect();
    let outline = TextOutline::new(
        &mut engine,
        TextDocument::new(lines).unwrap(),
        240.0,
        [36.0, 36.0],
    )
    .unwrap();
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Outline(outline.snapshot())],
        },
        &mut engine,
    )
    .unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 200],
        1.0,
        Duration::from_millis(500),
    );
    view.viewport.scale = 1.0;
    view.viewport.origin = [0.0; 2];
    let caret = view.editor.selection();
    let outline = view.editor.outlines()[0].id;
    let at = |offset| TextPosition {
        paragraph: 60,
        offset,
    };
    let note: Selection = [at(0), at(7)].into();
    let played = Some((outline, note));
    assert!(view.set_played(played).unwrap().moved);
    let rects = view.editor.outlines()[0].range_rects(note).unwrap();
    let top = view.editor.outlines()[0].origin()[1] + rects[0].y0 as f32;
    let bottom = view.editor.outlines()[0].origin()[1] + rects[0].y1 as f32;
    let shown = [top, bottom].map(|y| y + view.viewport.origin[1]);
    assert!(shown[0] >= 0.0 && shown[1] <= 200.0, "{shown:?}");
    assert_eq!(view.editor.selection(), caret);
    // Playing on within the same note leaves the view where the reader put it.
    view.viewport.origin[1] = 0.0;
    assert!(!view.set_played(played).unwrap().moved);
    assert_eq!(view.viewport.origin[1], 0.0);
}

/// The Spelling pane walks marked words from the caret in page order, wraps to the page's
/// top, and ends once none is left.
#[test]
fn the_spelling_pane_walks_marked_words_from_the_caret() {
    let mut engine = TextEngine::default();
    let document = TextDocument::new(
        ["the wrng word", "this is qwrt here"]
            .map(|text| Paragraph::new(text.into(), Default::default()))
            .into(),
    )
    .unwrap();
    let outline = TextOutline::new(&mut engine, document, 400.0, [36.0, 36.0]).unwrap();
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            rtl: false,
            color: None,
            rule_lines: None,
            definitions: Default::default(),
            objects: vec![onestore::page::PageObject::Outline(outline.snapshot())],
        },
        &mut engine,
    )
    .unwrap();
    let mut view = PageView::new(editor, engine, None, [800, 600], 1.0, Duration::ZERO);
    let spelling = crate::spelling::Spelling::new(
        Box::new(crate::spelling::tests::Fake),
        std::task::Waker::noop().clone(),
    );
    view.spelling = Some(spelling.clone());
    let caret = |paragraph, offset| [TextPosition { paragraph, offset }; 2].into();
    view.editor.select(caret(1, 17)).unwrap();
    let next = |view: &mut PageView| {
        view.next_correction()
            .unwrap()
            .map(|(_, correction)| correction.word)
    };
    assert_eq!(next(&mut view).as_deref(), Some("wrng"));
    assert_eq!(
        view.editor.selection().positions,
        [
            TextPosition {
                paragraph: 0,
                offset: 4
            },
            TextPosition {
                paragraph: 0,
                offset: 8
            }
        ]
    );
    spelling.ignore("wrng");
    assert_eq!(next(&mut view).as_deref(), Some("qwrt"));
    spelling.learn("qwrt");
    assert_eq!(next(&mut view), None);
}

/// Page `title` of `section`, with a view of it whose scene holds its pictures.
fn scene_view(section: &[u8], title: &str) -> PageView {
    let arena = onestore::Arena::default();
    let mut section = onestore::Section::open(&arena, section.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == title)
        .unwrap();
    let mut engine = TextEngine::default();
    let (scene, editor) = PageScene::from_page(section.page(space).unwrap(), &mut engine).unwrap();
    PageView::new(
        editor,
        engine,
        Some((scene, [0.0; 2])),
        [800, 600],
        1.0,
        Duration::from_millis(500),
    )
}

fn device(view: &PageView, point: [f32; 2]) -> [f32; 2] {
    [0, 1].map(|axis| point[axis] * view.viewport.scale + view.viewport.origin[axis])
}

/// A click selects a picture with a link and Ctrl+click (Command on macOS) follows it, as in
/// OneNote 2010, whose tooltip says so (`corpus/picture-link`).
#[test]
fn ctrl_click_follows_a_pictures_link() {
    let mut view = scene_view(
        include_bytes!("../../../../corpus/m6/native-features-01/notebook/Features.one"),
        "Image png",
    );
    let (id, [x, y], [width, height]) = view
        .editor
        .objects
        .iter()
        .find_map(|object| match object {
            crate::editor::page::Content::Image(image) => Some(image.id),
            _ => None,
        })
        .and_then(|id| {
            let (origin, size) = view.editor.image_placement(id)?;
            Some((id, origin, size))
        })
        .unwrap();
    assert_eq!(
        view.editor.picture_link(id),
        Some("https://example.invalid/image/png")
    );
    let _ = view
        .pointer_moved(device(&view, [x + width / 2.0, y + height / 2.0]))
        .unwrap();
    let response = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_released().unwrap();
    assert_eq!(response.request, None);
    assert_eq!(view.object_focus, Some(ObjectFocus::Image(id)));
    let _ = view
        .modifiers_changed(Modifiers {
            control: true,
            command: true,
            ..Modifiers::default()
        })
        .unwrap();
    assert_eq!(view.cursor(), Cursor::Pointer);
    let response = view.pointer_pressed(Instant::now()).unwrap();
    let _ = view.pointer_released().unwrap();
    assert_eq!(
        response.request,
        Some(Request::OpenLink(
            "https://example.invalid/image/png".into()
        ))
    );
}

/// A click on the check box OneNote 2010 draws beside a tagged picture on the page checks
/// it, and undo clears it (`corpus/object-tags`).
#[test]
fn a_check_box_beside_a_page_picture_checks_it() {
    let mut view = scene_view(
        include_bytes!("../../../../corpus/object-tags/native/notebook/files.one"),
        "Pictures and files",
    );
    let (id, [x, y], [_, height]) = view
        .editor
        .objects
        .iter()
        .find_map(|object| match object {
            crate::editor::page::Content::Image(image) if !image.tags.is_empty() => Some(image.id),
            _ => None,
        })
        .and_then(|id| {
            let (origin, size) = view.editor.image_placement(id)?;
            Some((id, origin, size))
        })
        .unwrap();
    let status = |view: &PageView| {
        view.editor
            .objects
            .iter()
            .find_map(|object| match object {
                crate::editor::page::Content::Image(image) if image.id == id => {
                    Some(image.tags[0].status)
                }
                _ => None,
            })
            .unwrap()
    };
    let _ = view
        .pointer_moved(device(&view, [x - 18.0, y + height / 2.0]))
        .unwrap();
    assert_eq!(view.cursor(), Cursor::Default);
    assert!(view.pointer_pressed(Instant::now()).unwrap().changed);
    let _ = view.pointer_released().unwrap();
    assert_eq!(status(&view) & 1, 1);
    assert!(view.editor.undo(&mut view.engine).unwrap());
    assert_eq!(status(&view) & 1, 0);
}

/// OneNote 2010 resizes a column from its right border: a column-resize pointer over it, the
/// table redrawn as it moves, and one edit on release.
#[test]
fn a_column_border_drags_with_a_resize_pointer_and_stores_on_release() {
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::new(
        &mut engine,
        TextDocument::new(vec![Paragraph::new(String::new(), Format::default())]).unwrap(),
        468.0,
    )
    .unwrap();
    editor.insert_table(&mut engine, 1, 2).unwrap();
    let mut view = PageView::new(
        editor,
        engine,
        None,
        [800, 600],
        1.0,
        Duration::from_millis(500),
    );
    let _ = view.editor.take_ops().unwrap();
    let outline = view.editor.active_outline();
    let cell = outline.shaped().tables[0].cells[0].clone();
    let origin = outline.origin();
    let (scale, offset) = (view.viewport.scale, view.viewport.origin);
    let device = |point: [f32; 2]| [0, 1].map(|axis| point[axis] * scale + offset[axis]);
    let border = [
        origin[0] + cell.rect[2],
        origin[1] + (cell.rect[1] + cell.rect[3]) / 2.0,
    ];
    let _ = view.pointer_moved(device(border)).unwrap();
    assert_eq!(view.cursor(), Cursor::ColResize);
    let _ = view.pointer_pressed(Instant::now()).unwrap();
    let to = device([border[0] + 50.0, border[1]]);
    assert!(view.pointer_moved(to).unwrap().changed);
    assert_eq!(view.cursor(), Cursor::ColResize);
    view.primitives(COLORS).unwrap();
    let onestore::page::ParagraphContent::Table(table) =
        &view.editor.active_outline().document().nodes()[0].content
    else {
        panic!()
    };
    assert!(!table.columns[0].locked);
    let _ = view.pointer_released().unwrap();
    let onestore::page::ParagraphContent::Table(table) =
        &view.editor.active_outline().document().nodes()[0].content
    else {
        panic!()
    };
    assert!(table.columns[0].locked);
    assert!((table.columns[0].width - (37.11 + 50.0)).abs() < 0.01);
    assert_eq!(view.editor.take_ops().unwrap().len(), 1);
}

/// The text of what a response asks the host to copy.
fn copied(response: Response) -> Option<String> {
    match response.request {
        Some(Request::Copy(clip)) => Some(clip.text()),
        _ => None,
    }
}
