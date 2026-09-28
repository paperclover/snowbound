use super::*;
use crate::document::TextDocument;
use crate::gpu::painted_layout;
use onestore::document::Format;
use onestore::page::Page;
use onestore::page::text::Paragraph;
use std::sync::Arc;

const COLORS: TextColors = TextColors {
    caret: [0.1, 0.3, 0.9, 1.0],
    selection: [0.7, 0.8, 1.0, 1.0],
    paper: crate::gpu::Paper::WHITE,
};

/// Paint for a view at `scale` device pixels per point on a display of `display_scale`.
pub(super) fn paint(show_caret: bool, scale: f32, display_scale: f32) -> Paint<'static> {
    Paint {
        caret: f32::from(u8::from(show_caret)),
        scale,
        pixel: display_scale / scale,
        colors: COLORS,
        visible: [f32::NEG_INFINITY, f32::INFINITY],
        chrome: true,
        found: &[],
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
        color: None,
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
        color: None,
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
    let moved = borders(Some(PointerFeedback::Move(second, [240.0, 50.0])));
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
        alt: None,
        background: false,
    });
    source.paragraphs.insert(1, picture);
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            color: None,
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
    editor.tab(&mut engine, true).unwrap();
    let mut primitives = Vec::new();
    append_outline(
        Some(&editor),
        editor.active_outline(),
        [24.0, 48.0],
        Paint {
            caret: 0.0,
            scale: 1.0,
            pixel: 1.0,
            colors: COLORS,
            visible: [f32::NEG_INFINITY, f32::INFINITY],
            chrome: true,
            found: &[],
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
        editor.active_outline(),
        [24.0, 48.0],
        Paint {
            caret: 0.0,
            scale: 1.0,
            pixel: 1.0,
            colors: COLORS,
            visible: [f32::NEG_INFINITY, f32::INFINITY],
            chrome: true,
            found: &[],
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
            color: None,
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
        color: None,
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
        alt: None,
        background: false,
    });
    source.paragraphs.insert(1, picture);
    let editor = CanvasEditor::from_page(
        Page {
            title: String::new(),
            identity: None,
            created: None,
            margin_origin: [36.0, 14.4],
            color: None,
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
        color: None,
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
    assert_eq!(
        view.copy(false).unwrap().request,
        Some(Request::Copy("B".into()))
    );
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
                if matches!(tag.icon, crate::outline::TagIcon::CheckBox { .. }) {
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
        .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
        .unwrap();
    check(&editor, 1.0);
    editor
        .format(&mut engine, Formatting::Tag(NoteTag::Question))
        .unwrap();
    check(&editor, 2.0);
    editor
        .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
        .unwrap();
    editor
        .format(&mut engine, Formatting::Tag(NoteTag::Question))
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
        .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
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
                .format(&mut engine, Formatting::Tag(NoteTag::ToDo))
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
            color: None,
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
