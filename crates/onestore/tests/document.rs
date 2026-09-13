use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use std::fs;

#[test]
fn documented_cell_shading_survives_native_2010_ignoring_its_display() {
    let bytes =
        fs::read("../../corpus/m6/cell-shading-control-01/input/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let colors: Vec<_> = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .flat_map(|r| r.nodes.values())
        .filter_map(|n| match n.kind {
            Kind::Cell { shading, .. } => shading,
            _ => None,
        })
        .collect();
    assert_eq!(colors, [0x0000ffff]);
}

#[test]
fn native_saved_collapse_default_preserves_descendants() {
    for (path, expected) in [
        (
            "../../corpus/m6/native-features-01/notebook/Features.one",
            1,
        ),
        (
            "../../corpus/m6/rust-collapse-control-01/input/notebook/Features.one",
            0,
        ),
    ] {
        let bytes = fs::read(path).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let mut parents = 0;
        for revision in document.spaces.values().flat_map(|s| s.revisions.values()) {
            for node in revision.nodes.values() {
                if let Kind::Paragraph { collapse_state, .. } = node.kind
                    && node.content.iter().any(|id| {
                        matches!(&revision.nodes[id].kind, Kind::RichText { text, .. } if text == "Collapsed parent")
                    })
                {
                        assert_eq!(collapse_state, Some(expected));
                        assert_eq!(node.children.len(), 1);
                        assert_eq!(revision.nodes[&node.children[0]].children.len(), 1);
                        parents += 1;
                }
            }
        }
        assert_eq!(parents, 1);
    }
}

#[test]
fn native_math_preserves_run_data_and_distinguishes_plain_text() {
    let bytes = fs::read("../../corpus/m6/native-math-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut equations = 0;
    let mut inline = 0;
    for revision in document.spaces.values().flat_map(|s| s.revisions.values()) {
        for (id, node) in &revision.nodes {
            let Kind::RichText { text, runs, .. } = &node.kind else {
                continue;
            };
            let resolved = revision.text_runs(*id).unwrap();
            if text.contains('\u{fdd0}') {
                equations += 1;
                assert!(resolved.iter().all(|r| r.format.math == Some(true)));
                let expected = if text.contains('𝑥') {
                    assert_eq!(
                        text,
                        "\u{fdd0}𝑥\u{fdee}2\u{fdef}+\u{fdd0}𝑦\u{fdee}2\u{fdef}=\u{fdd0}𝑧\u{fdee}2\u{fdef}"
                    );
                    11
                } else {
                    assert_eq!(text, "\u{fdd0}𝑎+𝑏\u{fdee}𝑐+𝑑\u{fdef}");
                    3
                };
                assert_eq!(runs.len(), expected);
                for (i, run) in runs.iter().enumerate() {
                    assert_eq!(run.extra_set, Some(i + 1));
                    assert!(!node.extra[run.extra_set.unwrap()].is_empty());
                }
            }
            if text == "Inline 𝛼+𝛽" {
                inline += 1;
                assert_eq!(resolved.len(), 2);
                assert_eq!(resolved[0].text, "Inline ");
                assert_eq!(resolved[0].format.math, Some(false));
                assert_eq!(resolved[1].text, "𝛼+𝛽");
                assert_eq!(resolved[1].format.math, Some(true));
                assert!(runs.iter().all(|r| r.extra_set.is_none()));
            }
        }
    }
    assert_eq!((equations, inline), (2, 1));
}

#[test]
fn native_origin_control_retains_source_coordinate_frame() {
    let bytes =
        fs::read("../../corpus/m6/native-origin-controls-01/input/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (revision, page) = document
        .spaces
        .values()
        .flat_map(|space| space.revisions.values())
        .find_map(|revision| {
            revision
                .nodes
                .values()
                .find(|node| matches!(node.kind, Kind::Page { .. }))
                .map(|page| (revision, page))
        })
        .unwrap();
    let Kind::Page {
        margin_origin_x,
        margin_origin_y,
        rtl,
        ..
    } = page.kind
    else {
        unreachable!()
    };
    assert_eq!(margin_origin_x, Some(108.0));
    assert_eq!(margin_origin_y, Some(-36.0));
    assert_ne!(rtl, Some(true));
    let outline = &revision.nodes[&page.children[0]];
    assert_eq!(outline.layout.x, Some(36.0));
    assert!((outline.layout.y.unwrap() - 14.4).abs() < 0.002);
}

#[test]
fn native_rtl_table_retains_visual_column_order() {
    let bytes =
        fs::read("../../corpus/m6/native-page-direction-03/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let revision = document
        .spaces
        .values()
        .flat_map(|space| space.revisions.values())
        .find(|revision| {
            revision.nodes.values().any(|node| {
                matches!(
                    node.kind,
                    Kind::Page {
                        rtl: Some(true),
                        ..
                    }
                )
            })
        })
        .unwrap();
    let table = revision
        .nodes
        .values()
        .find(|node| matches!(node.kind, Kind::Table { .. }))
        .unwrap();
    let Kind::Table { widths, locked, .. } = &table.kind else {
        unreachable!()
    };
    assert_eq!(widths, &[144.0, 96.0]);
    assert_eq!(locked, &[true, true]);
    let row = &revision.nodes[&table.children[0]];
    let text: Vec<_> = row
        .children
        .iter()
        .map(|id| {
            let paragraph = &revision.nodes[&revision.nodes[id].children[0]];
            revision
                .text_runs(paragraph.content[0])
                .unwrap()
                .iter()
                .map(|run| run.text)
                .collect::<String>()
        })
        .collect();
    assert_eq!(text, ["Right cell", "Left cell"]);
}

#[test]
fn native_default_template_has_a_named_page_relationship() {
    let bytes =
        fs::read("../../corpus/m6/native-template-controls-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let root = &document.spaces[&document.root];
    let root = &root.revisions[&root.contexts[&ExGuid::default()]];
    let Kind::Section {
        default_template: Some(template),
    } = root.nodes[&root.roots[&1]].kind
    else {
        panic!("Missing default template")
    };
    let template = &document.spaces[&template];
    let template = &template.revisions[&template.contexts[&ExGuid::default()]];
    let Kind::TemplateMetadata { name } = &template.nodes[&template.roots[&2]].kind else {
        panic!("Missing template metadata")
    };
    assert_eq!(name.as_deref(), Some("Hearts"));
    let manifest = &template.nodes[&template.roots[&1]];
    assert_eq!(manifest.content.len(), 1);
    assert!(matches!(
        template.nodes[&manifest.content[0]].kind,
        Kind::Page { .. }
    ));
}

#[test]
fn native_paragraph_breaks_preserve_every_stored_character() {
    let bytes =
        fs::read("../../corpus/m6/native-break-controls-02/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (revision, outline) = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .find_map(|revision| {
            revision
                .nodes
                .values()
                .find(|node| matches!(node.kind, Kind::Outline { .. }) && node.children.len() == 24)
                .map(|node| (revision, node))
        })
        .unwrap();
    let expected = [
        "A", "A\r", "A\r\r", "\rA", "\rA\r", "\r", "\r\r", "A\rB\r", "A\rB\r", "AB", "A\rB\r",
        "A\rB\r",
    ];
    for (case, paragraph) in outline.children.iter().enumerate() {
        let runs = revision
            .text_runs(revision.nodes[paragraph].content[0])
            .unwrap();
        let visible: Vec<_> = runs
            .iter()
            .filter(|r| r.format.hidden != Some(true))
            .collect();
        assert_eq!(
            visible.iter().map(|r| r.text).collect::<String>(),
            expected[case % 12],
            "case {case}"
        );
        for run in visible {
            for character in run.text.chars() {
                assert_eq!(
                    run.link.is_some(),
                    case >= 12 && character != '\r',
                    "case {case}"
                );
            }
        }
    }
}

#[test]
fn referenced_history_contexts_require_history_roots() {
    let bytes = fs::read("../../corpus/m6/native-features-01/notebook/Features.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let mut index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, context, current) = document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            let current = space.contexts[&ExGuid::default()];
            let revision = &space.revisions[&current];
            match revision.nodes[&revision.roots[&1]].kind {
                Kind::Manifest {
                    history: Some(context),
                } => Some((*sid, context, current)),
                _ => None,
            }
        })
        .unwrap();
    let space = &document.spaces[&sid];
    let history = &space.revisions[&space.contexts[&context]];
    assert!(matches!(
        history.nodes[&history.roots[&1]].kind,
        Kind::VersionHistory
    ));
    assert!(history.nodes[&history.roots[&1]].children.is_empty());
    index
        .spaces
        .get_mut(&sid)
        .unwrap()
        .labels
        .insert((context, 1), current);
    assert_eq!(
        Document::parse(&index).unwrap_err().message,
        "History context does not resolve to version history"
    );
    index
        .spaces
        .get_mut(&sid)
        .unwrap()
        .labels
        .remove(&(context, 1));
    assert_eq!(
        Document::parse(&index).unwrap_err().message,
        "Document context has no revision"
    );
}

#[test]
fn native_hyperlink_boundaries_preserve_whitespace_and_literal_labels() {
    let bytes = fs::read("../../corpus/m6/native-link-controls-01/notebook/Links.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, outline) = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .find_map(|space| {
            space.nodes.values().find_map(|node| {
                (matches!(node.kind, Kind::Outline { .. }) && node.children.len() == 10)
                    .then_some((space, node))
            })
        })
        .unwrap();
    let expected = [
        (" label ", "label "),
        ("\tlabel\t", "label\t"),
        ("\u{a0}label\u{a0}", "\u{a0}label\u{a0}"),
        (" \tlabel \t", "label \t"),
        ("label", "label"),
        ("  ", ""),
        ("label", "label"),
        (" ]]> end", "]]> end"),
        ("<b> & \"quoted\"", "<b> & \"quoted\""),
        ("\rlabel\r", "label"),
    ];
    for (number, (paragraph, (text, linked))) in outline.children.iter().zip(expected).enumerate() {
        let runs = space.text_runs(space.nodes[paragraph].content[0]).unwrap();
        let visible: Vec<_> = runs
            .iter()
            .filter(|r| r.format.hidden != Some(true))
            .collect();
        assert_eq!(visible.iter().map(|r| r.text).collect::<String>(), text);
        assert_eq!(
            visible
                .iter()
                .filter(|r| r.link.is_some())
                .map(|r| r.text)
                .collect::<String>(),
            linked
        );
        let target = format!("https://example.invalid/label/{number}");
        for run in visible {
            if let Some(link) = run.link {
                assert_eq!(link, target);
            }
            assert_eq!(run.format.bold, Some(true));
            assert_eq!(run.format.font_size, Some(14.0));
            assert_eq!(run.format.underline, Some(false));
            assert_eq!(run.format.color, Some(0xff000000));
        }
    }
}

#[test]
fn native_unicode_table_assets_and_coordinates_are_typed() {
    let bytes =
        fs::read("../../corpus/native/20260905-05/snapshots/07-table/notebook/synthetic.one")
            .unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let nodes: Vec<_> = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .flat_map(|r| r.nodes.values())
        .collect();
    assert!(nodes.iter().any(|n| matches!(&n.kind, Kind::RichText { text, .. } if text == "Fictitious: café, 東京, مرحبا")));
    let table = nodes
        .iter()
        .find(|n| matches!(n.kind, Kind::Table { .. }))
        .unwrap();
    let Kind::Table {
        rows,
        columns,
        widths,
        ..
    } = &table.kind
    else {
        unreachable!()
    };
    assert_eq!(
        (*rows, *columns, table.children.len(), widths.len()),
        (Some(1), Some(2), 1, 2)
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|n| matches!(n.kind, Kind::Cell { .. }))
            .count(),
        2
    );
    assert_eq!(
        nodes
            .iter()
            .filter(|n| matches!(n.kind, Kind::Attachment { .. }))
            .count(),
        1
    );
    assert!(nodes.iter().any(|n| matches!(&n.kind, Kind::File { payload: Some(data), .. } if data.starts_with(b"\x89PNG"))));
    assert!(nodes.iter().any(|n| matches!(n.kind, Kind::Outline { .. })
        && n.layout.x == Some(144.0)
        && n.layout.y == Some(96.0)));
    for node in nodes {
        if let Kind::RichText { text, runs, .. } = &node.kind {
            assert_eq!(runs.first().unwrap().start, 0);
            assert_eq!(
                runs.last().unwrap().end as usize,
                text.encode_utf16().count()
            );
        }
    }
}

#[test]
fn encrypted_content_is_accounted_without_fabricated_plaintext() {
    for path in [
        "../../corpus/native-encrypted/encrypted-01/notebook/synthetic.one",
        "../../corpus/native-protected-boundaries/notebook/synthetic.one",
    ] {
        let bytes = fs::read(path).unwrap();
        let store = Store::parse(&bytes).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        assert!(!document.spaces.is_empty());
        let nodes: Vec<_> = document
            .spaces
            .values()
            .flat_map(|s| s.revisions.values())
            .flat_map(|r| r.nodes.values())
            .collect();
        assert!(!nodes.is_empty());
        for node in nodes {
            assert!(matches!(node.kind, Kind::Encrypted { ciphertext } if !ciphertext.is_empty()));
        }
    }
}

#[test]
fn rust_unicode_and_native_latin1_use_utf16_run_offsets() {
    let text = "A😀e\u{301}東京";
    let bytes = onestore::create_section("unicode.one", text, "Fictitious").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let node = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .flat_map(|r| r.nodes.values())
        .find(|n| matches!(n.kind, Kind::RichText { .. }))
        .unwrap();
    let Kind::RichText {
        text: actual, runs, ..
    } = &node.kind
    else {
        unreachable!()
    };
    assert_eq!(actual, text);
    assert_eq!(runs[0].end, 7);
    let bytes = fs::read("../../corpus/m6/ansi/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let expected = format!(
        "Extended bytes: {}",
        (128u8..=255)
            .map(|b| char::from(b).to_string())
            .collect::<Vec<_>>()
            .join(" ")
    );
    assert!(
        document
            .spaces
            .values()
            .flat_map(|s| s.revisions.values())
            .flat_map(|r| r.nodes.values())
            .any(|n| matches!(&n.kind, Kind::RichText { text, .. } if text == &expected))
    );
}

#[test]
fn toc_preserves_section_order_and_filename() {
    let bytes = onestore::create_table_of_contents(
        "Open Notebook.onetoc2",
        &[("z.one", [1; 16]), ("a.one", [2; 16])],
    )
    .unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = &document.spaces[&document.root];
    let space = &space.revisions[&space.contexts[&ExGuid::default()]];
    let root = &space.nodes[&space.roots[&1]];
    let Kind::Toc { entries, .. } = &root.kind else {
        panic!("Missing TOC")
    };
    let names: Vec<_> = entries
        .iter()
        .map(|id| match &space.nodes[id].kind {
            Kind::Toc { filename, .. } => filename.as_deref().unwrap(),
            _ => panic!("Missing TOC entry"),
        })
        .collect();
    assert_eq!(names, ["z.one", "a.one"]);
    assert_ne!(document.root, ExGuid::default());
}

#[test]
fn native_printouts_tasks_and_recording_associations_are_typed() {
    let bytes = fs::read("../../corpus/m6/native-features-01/notebook/Features.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut roles = Vec::new();
    let mut task = false;
    let mut recording = false;
    let mut annotation = false;
    let mut decimal = false;
    let mut inherited_strike = false;
    let media = [
        0x57, 0x8e, 0x41, 0x9a, 0x64, 0x7f, 0xc5, 0x40, 0x87, 0xd0, 0x8a, 0x8f, 0x7c, 0x1e, 0x54,
        0x22,
    ];
    for space in document.spaces.values().flat_map(|s| s.revisions.values()) {
        for (oid, node) in &space.nodes {
            match &node.kind {
                Kind::Image {
                    alt,
                    background,
                    printout,
                    ..
                } if alt.as_deref() == Some("Role pattern") => {
                    let border = if *printout == Some(true) { 1.44 } else { 0.0 };
                    assert!((node.layout.max_width.unwrap() + border - 288.0).abs() < 0.002);
                    assert!((node.layout.max_height.unwrap() + border - 192.0).abs() < 0.002);
                    roles.push((*background, *printout));
                }
                Kind::Attachment {
                    filename,
                    recording_id,
                    ..
                } if filename.as_deref() == Some("silence.wav") => {
                    assert_eq!(*recording_id, Some(media));
                    recording = true;
                }
                Kind::RichText { text, .. } if text == "Recording annotation" => {
                    assert_eq!(node.media_ids, [media]);
                    assert_eq!(node.media_time_ms, Some(500));
                    annotation = true;
                }
                Kind::RichText { text, .. } if text == "Disabled standalone task" => {
                    assert_eq!(node.tags.len(), 1);
                    let tag = &node.tags[0];
                    assert_eq!(
                        (tag.definition, tag.action_type, tag.status),
                        (None, Some(100), 6)
                    );
                    assert_eq!((tag.start, tag.due), (Some(1262390400), Some(1262476800)));
                    assert_eq!(tag.completed, Some(0));
                    assert_eq!(tag.task_id.unwrap()[..4], [0xe3, 0x40, 0xaf, 0x45]);
                    task = true;
                }
                Kind::RichText { text, .. } if text == "Inherited strike and spacing" => {
                    let runs = space.text_runs(*oid).unwrap();
                    assert_eq!(runs[0].format.strike, Some(true));
                    assert_eq!(runs[0].format.space_before, Some(9.0));
                    assert_eq!(runs[0].format.space_after, Some(4.5));
                    inherited_strike = true;
                }
                Kind::List { format, .. } if format.as_deref() == Some("\u{fffd}\0") => {
                    decimal = true;
                }
                _ => {}
            }
        }
    }
    roles.sort();
    assert_eq!(
        roles,
        [
            (Some(false), Some(false)),
            (Some(false), Some(true)),
            (Some(true), Some(false)),
            (Some(true), Some(true))
        ]
    );
    assert!(task && recording && annotation && decimal && inherited_strike);
}

#[test]
fn native_partial_black_highlight_and_inline_hyperlink_fields_survive() {
    let bytes = fs::read("../../corpus/m6/native-probes-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut black = false;
    let mut link = false;
    for space in document.spaces.values().flat_map(|s| s.revisions.values()) {
        for (oid, node) in &space.nodes {
            if let Kind::RichText { text, runs, .. } = &node.kind {
                if text == "Before middle after" {
                    let highlighted: Vec<_> = runs
                        .iter()
                        .filter(|run| {
                            run.format
                                .is_some_and(|id| space.nodes[&id].format.highlight == Some(0))
                        })
                        .collect();
                    assert_eq!(highlighted.len(), 1);
                    assert_eq!((highlighted[0].start, highlighted[0].end), (7, 13));
                    black = true;
                }
                if text.starts_with("Before \u{fddf}HYPERLINK") {
                    let units: Vec<_> = text.encode_utf16().collect();
                    let mut visible = String::new();
                    let mut hidden = String::new();
                    for run in runs {
                        let fragment =
                            String::from_utf16(&units[run.start as usize..run.end as usize])
                                .unwrap();
                        if run
                            .format
                            .is_some_and(|id| space.nodes[&id].format.hidden == Some(true))
                        {
                            hidden.push_str(&fragment);
                        } else {
                            visible.push_str(&fragment);
                        }
                    }
                    assert_eq!(visible, "Before Link label after");
                    let resolved = space.text_runs(*oid).unwrap();
                    let links: Vec<_> = resolved
                        .iter()
                        .filter_map(|r| r.link.map(|url| (r.text, url, r.format.bold)))
                        .collect();
                    assert_eq!(
                        links,
                        [(
                            "Link label",
                            "https://example.invalid/fixture?x=1&y=2",
                            Some(true)
                        )]
                    );
                    assert_eq!(
                        resolved
                            .iter()
                            .filter(|r| r.format.hidden != Some(true))
                            .map(|r| r.text)
                            .collect::<String>(),
                        visible
                    );
                    assert_eq!(
                        hidden,
                        "\u{fddf}HYPERLINK \"https://example.invalid/fixture?x=1&y=2\""
                    );
                    link = true;
                }
            }
        }
    }
    assert!(black && link);
}

#[test]
fn malformed_run_boundaries_fail_after_valid_storage_decoding() {
    let bytes = fs::read("../../corpus/m6/native-probes-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (sid, rid, oid) = document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            space.revisions.iter().find_map(|(rid, revision)| {
                revision.nodes.iter().find_map(|(oid, node)| {
                    matches!(&node.kind, Kind::RichText { text, .. } if text.starts_with("BMP "))
                        .then_some((*sid, *rid, *oid))
                })
            })
        })
        .unwrap();
    let revision = index.resolve(sid, rid).unwrap();
    let onestore::ObjectData::Properties(properties) = revision.objects[&oid].data else {
        unreachable!()
    };
    let properties = onestore::PropertySets::parse(properties).unwrap();
    let onestore::Value::Bytes(boundaries) = properties.sets[0]
        .iter()
        .find(|p| p.id == 0x1c001e12)
        .unwrap()
        .value
    else {
        unreachable!()
    };
    for invalid in [5u32, u32::MAX] {
        let mut changed = boundaries.to_vec();
        changed[..4].copy_from_slice(&invalid.to_le_bytes());
        let mutated =
            onestore::replace_property_bytes(&bytes, sid, oid, 0x1c001e12, &changed).unwrap();
        let store = Store::parse(&mutated).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let error = Document::parse(&index).unwrap_err();
        assert_eq!(
            error.message,
            if invalid == 5 {
                "Text-run boundary splits a surrogate pair"
            } else {
                "Text-run indices exceed text or are out of order"
            }
        );
    }
}

#[test]
fn native_lists_locked_columns_and_named_styles_are_preserved() {
    let bytes = fs::read("../../corpus/m6/native-structure-01/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut found = (false, false, false);
    for space in document.spaces.values().flat_map(|s| s.revisions.values()) {
        for (oid, node) in &space.nodes {
            match &node.kind {
                Kind::Table {
                    rows,
                    columns,
                    locked,
                    widths,
                    ..
                } => {
                    assert_eq!((*rows, *columns), (Some(3), Some(2)));
                    assert_eq!(locked, &[true, false]);
                    assert_eq!(widths[0], 120.0);
                    assert!(widths[1] > 37.0 && widths[1] < 38.0);
                    found.0 = true;
                }
                Kind::List {
                    format,
                    restart: Some(3),
                    bullet,
                    ..
                } => {
                    assert_eq!(format.as_deref(), Some("\u{fffd}\u{1}."));
                    assert_eq!(*bullet, None);
                    found.1 = true;
                }
                Kind::RichText { text, .. } if text == "Named paragraph style" => {
                    let runs = space.text_runs(*oid).unwrap();
                    assert_eq!(runs[0].format.bold, Some(true));
                    assert_eq!(runs[0].format.font_size, Some(20.0));
                    assert_eq!(runs[0].format.space_before, Some(288.0));
                    assert_eq!(runs[0].format.space_after, Some(144.0));
                    assert_eq!(runs[0].format.alignment, Some(2));
                    found.2 = true;
                }
                _ => {}
            }
        }
    }
    assert_eq!(found, (true, true, true));
    let (revision, outer) = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .find_map(|revision| {
            revision
                .nodes
                .values()
                .find(|node| {
                    matches!(node.kind, Kind::Paragraph { .. }) && !node.children.is_empty()
                })
                .map(|node| (revision, node))
        })
        .unwrap();
    assert_eq!(outer.child_level, Some(1));
    assert_eq!(
        revision.text_runs(outer.content[0]).unwrap()[0].text,
        "Outer bullet"
    );
    assert_eq!(outer.children.len(), 1);
    let nested = &revision.nodes[&outer.children[0]];
    assert_eq!(
        revision.text_runs(nested.content[0]).unwrap()[0].text,
        "Nested bullet"
    );
}

#[test]
fn native_conflict_relationship_and_opaque_ink_are_retained() {
    let bytes =
        fs::read("../../corpus/collaboration/round-01/offline/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (conflict_id, conflict) = document
        .spaces
        .iter()
        .find_map(|(sid, space)| {
            let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
            revision.roots.get(&2).and_then(|root| {
                matches!(revision.nodes[root].kind, Kind::ConflictMetadata { .. })
                    .then_some((sid, revision))
            })
        })
        .unwrap();
    let Kind::ConflictMetadata {
        title,
        author,
        level,
    } = &conflict.nodes[&conflict.roots[&2]].kind
    else {
        unreachable!()
    };
    assert_eq!(title.as_deref(), Some("Native edited while disconnected."));
    assert_eq!(author.as_deref(), Some("snow"));
    assert_eq!(*level, Some(1));
    assert!(
        document
            .spaces
            .iter()
            .any(|(sid, space)| sid != conflict_id && {
                let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
                revision.nodes[&revision.roots[&1]]
                    .spaces
                    .contains(conflict_id)
            })
    );
    let bytes = fs::read("../../corpus/native-ink/cold-ui-ink/notebook/synthetic.one").unwrap();
    let store = Store::parse(&bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let ink: Vec<_> = document
        .spaces
        .values()
        .flat_map(|s| s.revisions.values())
        .flat_map(|r| r.nodes.values())
        .filter(|node| matches!(node.kind, Kind::Ink { .. }))
        .collect();
    assert_eq!(ink.len(), 2);
    assert!(
        ink.iter()
            .all(|node| node.extra.iter().any(|set| !set.is_empty()))
    );
}
