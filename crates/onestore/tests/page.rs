use onestore::{
    ExGuid, PageCreation, RevisionIndex, Store,
    document::{Document, Kind},
    op::{PageOp, SectionOp},
    page::{Page, PageObject},
};

#[path = "support/current.rs"]
mod current;
#[path = "support/disk.rs"]
mod disk;
#[path = "support/ops.rs"]
mod ops;

const SOURCE: &[u8] =
    include_bytes!("../../../corpus/page-lifecycle/03-renamed/notebook/Lifecycle.one");

#[test]
fn native_section_accepts_empty_and_titled_pages_with_preserved_history() {
    let original = Store::parse(SOURCE).unwrap();
    let old_index = RevisionIndex::parse(&original).unwrap();
    let original_pages = Document::parse(&old_index).unwrap().pages().unwrap();
    let mut expected = original_pages.clone();
    let mut source = SOURCE.to_vec();
    for (ordinal, title) in [None, Some(""), Some("New 🦋 é"), Some("New 🦋 é")]
        .into_iter()
        .enumerate()
    {
        let before = (ordinal == 1).then_some(original_pages[0].0);
        let intent = PageCreation::new(before, title, "Rust author").unwrap();
        let serialized = serde_json::to_vec(&intent).unwrap();
        let retained = serde_json::from_slice(&serialized).unwrap();
        assert_eq!(intent, retained);
        let prepared = ops::section_op(&source, SectionOp::Create(retained.clone())).unwrap();
        let store = Store::parse(prepared.as_bytes()).unwrap();
        assert_eq!(
            store.header.transaction_count,
            original.header.transaction_count + ordinal as u32 + 1
        );
        assert_eq!(store.header.file_id, original.header.file_id);
        let index = RevisionIndex::parse(&store).unwrap();
        index.validate_current().unwrap();
        let document = Document::parse(&index).unwrap();
        let at = if ordinal == 1 { 0 } else { expected.len() };
        expected.insert(at, (intent.space(), intent.object()));
        assert_eq!(document.pages().unwrap(), expected);
        let view = &document.spaces[&intent.space()];
        let view = &view.revisions[&view.contexts[&ExGuid::default()]];
        assert!(view.nodes[&intent.object()].children.is_empty());
        assert_eq!(
            view.nodes[&intent.object()].structure.len(),
            usize::from(title.is_some())
        );
        let Kind::Metadata {
            title: actual,
            level,
        } = &view.nodes[&view.roots[&2]].kind
        else {
            panic!()
        };
        assert_eq!(actual.as_deref(), Some(title.unwrap_or_default()));
        assert_eq!(*level, Some(1));
        if let Some(text) = intent.title_object() {
            let Kind::RichText {
                text, boilerplate, ..
            } = &view.nodes[&text].kind
            else {
                panic!()
            };
            assert_eq!(text, title.unwrap());
            assert!(!boilerplate);
        }
        for (sid, space) in &old_index.spaces {
            for rid in space.revisions.keys() {
                assert_eq!(
                    format!("{:?}", old_index.resolve(*sid, *rid).unwrap()),
                    format!("{:?}", index.resolve(*sid, *rid).unwrap())
                );
            }
            if *sid != old_index.root {
                assert_eq!(space.labels, index.spaces[sid].labels);
            }
        }
        assert!(ops::section_op(prepared.as_bytes(), SectionOp::Create(intent.clone())).is_err());
        source = prepared.as_bytes().to_vec();
    }
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_CREATION_OUTPUT") {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(std::path::Path::new(&output).join("Lifecycle.one"), &source).unwrap();
    }
}

#[test]
fn created_pages_support_title_edits_and_body_insertion() {
    let mut source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    for title in [None, Some(""), Some("Explicit 🦋 é")] {
        let intent = PageCreation::new(None, title, "Author").unwrap();
        source = ops::section_op(&source, SectionOp::Create(intent.clone()))
            .unwrap()
            .as_bytes()
            .to_vec();
        let (add, ..) = ops::new_outline(36.0, 36.0, "Body 🦀 é");
        source = ops::page_op(&source, intent.space(), add)
            .unwrap()
            .as_bytes()
            .to_vec();
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let view = &document.spaces[&intent.space()];
        let view = &view.revisions[&view.contexts[&ExGuid::default()]];
        let Kind::Metadata { title: actual, .. } = &view.nodes[&view.roots[&2]].kind else {
            panic!()
        };
        assert_eq!(
            actual.as_deref(),
            Some(title.filter(|s| !s.is_empty()).unwrap_or("Body 🦀 é"))
        );
        if let Some(text) = intent.title_object() {
            source = ops::page_op(&source, intent.space(), PageOp::Text { text, range: 0..title.unwrap().encode_utf16().count() as u32, with: "Renamed".into() })
            .unwrap()
            .as_bytes()
            .to_vec();
            let store = Store::parse(&source).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            let view = &document.spaces[&intent.space()];
            let view = &view.revisions[&view.contexts[&ExGuid::default()]];
            let Kind::Metadata { title, .. } = &view.nodes[&view.roots[&2]].kind else {
                panic!()
            };
            assert_eq!(title.as_deref(), Some("Renamed"));
        }
    }
}

/// OneNote 2010 draws level-1 text 27 pt inside an outline whose table stops at entry 0.
#[test]
fn created_body_outlines_carry_the_indentation_table_onenote_writes() {
    let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, page) = document.pages().unwrap()[0];
    let [first] = document.active(space).unwrap().nodes[&page].children[..] else {
        panic!()
    };
    let (add, insertion, _) = ops::new_outline(72.0, 144.0, "Second");
    let written = ops::page_op(&source, space, add).unwrap();
    let store = Store::parse(written.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let view = document.active(space).unwrap();
    for outline in [first, insertion] {
        let Kind::Outline { indents } = &view.nodes[&outline].kind else {
            panic!()
        };
        assert_eq!(indents, &[18.0, 0.0, 27.0, 27.0]);
    }
}

#[test]
fn a_created_title_outline_is_stored_and_read_as_onenote_stores_it() {
    let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    let intent = PageCreation::new(None, Some("Title"), "Author").unwrap();
    let prepared = ops::section_op(&source, SectionOp::Create(intent.clone())).unwrap();
    let store = Store::parse(prepared.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let view = document.active(intent.space()).unwrap();
    let [title] = view.nodes[&intent.object()].structure[..] else {
        panic!()
    };
    let [outline] = view.nodes[&title].children[..] else {
        panic!()
    };
    let [paragraph] = view.nodes[&outline].children[..] else {
        panic!()
    };
    let Kind::Outline { indents } = &view.nodes[&outline].kind else {
        panic!()
    };
    assert_eq!(indents, &[18.0, 0.0, 27.0, 27.0]);
    let flags = |id| -> Vec<u32> {
        view.nodes[&id].extra[0]
            .iter()
            .map(|field| field.id)
            .filter(|id| id >> 24 == 0x88)
            .collect()
    };
    assert_eq!(
        flags(outline),
        [0x88001cf9, 0x88001cb2, 0x88001cb4, 0x88001c91, 0x88001cff]
    );
    assert_eq!(flags(paragraph), [0x88001cb2, 0x88001cb4]);
    let page = Page::from_space(&document, intent.space()).unwrap();
    let [PageObject::Title(title)] = &page.objects[..] else {
        panic!()
    };
    let [outline] = &title.outlines[..] else {
        panic!()
    };
    assert!(outline.title);
    assert_eq!(outline.indents, [18.0, 0.0, 27.0, 27.0]);
    assert_eq!(outline.min_width, Some(162.0));
    assert_eq!(outline.layout.max_height, Some(21.6));
}

#[test]
fn invalid_page_intents_and_nonleading_anchors_reject_before_publication() {
    for title in ["a\nb", "\r", "\0", "\u{fffc}", "\u{fddf}"] {
        assert!(PageCreation::new(None, Some(title), "Author").is_err());
    }
    assert!(PageCreation::new(None, None, "\0").is_err());
    assert!(PageCreation::new(Some(ExGuid::default()), None, "Author").is_err());
    let source = include_bytes!("../../../corpus/page-lifecycle/04-nested/notebook/Lifecycle.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let pages = Document::parse(&index).unwrap().pages().unwrap();
    let intent = PageCreation::new(Some(pages[4].0), Some("Child"), "Author").unwrap();
    assert!(ops::section_op(source, SectionOp::Create(intent.clone())).is_err());
    let mut value = serde_json::to_value(PageCreation::new(None, None, "Author").unwrap()).unwrap();
    value["guid"] = serde_json::to_value([0_u8; 16]).unwrap();
    let invalid: PageCreation = serde_json::from_value(value.clone()).unwrap();
    assert!(ops::section_op(source, SectionOp::Create(invalid.clone())).is_err());
    value["unknown"] = true.into();
    assert!(serde_json::from_value::<PageCreation>(value).is_err());
}

#[test]
fn new_space_and_section_entry_publish_as_one_complete_action() {
    let source = onestore::create_section("pages.one", "Original", "Author").unwrap();
    let intent = PageCreation::new(None, Some("New 🦋 é"), "Author").unwrap();
    let prepared = ops::section_op(&source, SectionOp::Create(intent.clone())).unwrap();
    let old = current::current(&source);
    let new = current::current(prepared.as_bytes());
    assert_eq!(new.len(), old.len() + 1);
    for write_limit in [1, 17, 4096] {
        let mut complete = disk::Disk {
            visible: source.clone(),
            durable: source.clone(),
            operation: 0,
            fail_at: None,
            write_limit,
            random: 1954,
        };
        prepared.commit(&mut complete).unwrap();
        assert_eq!(complete.durable, prepared.as_bytes());
        for fail_at in 1..=complete.operation {
            let mut interrupted = disk::Disk {
                visible: source.clone(),
                durable: source.clone(),
                operation: 0,
                fail_at: Some(fail_at),
                write_limit,
                random: 1954 + fail_at as u64,
            };
            prepared.commit(&mut interrupted).unwrap_err();
            let observed = current::current(&interrupted.durable);
            assert!(
                observed == old || observed == new,
                "{write_limit}:{fail_at}"
            );
        }
    }
}

#[test]
fn repeated_page_creation_crosses_root_fragments_counters_and_section_checkpoints() {
    let mut source = onestore::create_section("PageStress.one", "Original", "Author").unwrap();
    let original = source.clone();
    let original_store = Store::parse(&original).unwrap();
    let original_index = RevisionIndex::parse(&original_store).unwrap();
    let mut expected = Document::parse(&original_index).unwrap().pages().unwrap();
    let mut checkpoints = 0;
    for step in 1..=520 {
        let title = format!("Page {} 🦋 é", step % 7);
        let intent = PageCreation::new(
            None,
            (step % 2 != 0).then_some(title.as_str()),
            "Stress author",
        )
        .unwrap();
        let prepared = ops::section_op(&source, SectionOp::Create(intent.clone())).unwrap();
        let mut unpublished = prepared.as_bytes().to_vec();
        unpublished[96..100].copy_from_slice(&source[96..100]);
        let old_store = Store::parse(&unpublished).unwrap();
        let old_index = RevisionIndex::parse(&old_store).unwrap();
        assert_eq!(
            Document::parse(&old_index).unwrap().pages().unwrap(),
            expected
        );
        let store = Store::parse(prepared.as_bytes()).unwrap();
        assert_eq!(store.header.root, original_store.header.root);
        assert_eq!(
            store.header.transaction_count,
            original_store.header.transaction_count + step
        );
        let index = RevisionIndex::parse(&store).unwrap();
        assert_eq!(
            index.spaces.len(),
            original_index.spaces.len() + step as usize
        );
        expected.push((intent.space(), intent.object()));
        assert_eq!(Document::parse(&index).unwrap().pages().unwrap(), expected);
        let section = &index.spaces[&index.root];
        let rid = section.labels[&(ExGuid::default(), 1)];
        checkpoints += usize::from(section.revisions[&rid].dependency.is_none());
        for (sid, space) in &original_index.spaces {
            for rid in space.revisions.keys() {
                assert_eq!(
                    format!("{:?}", original_index.resolve(*sid, *rid).unwrap()),
                    format!("{:?}", index.resolve(*sid, *rid).unwrap())
                );
            }
        }
        source = prepared.as_bytes().to_vec();
    }
    assert_eq!(checkpoints, 1);
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_STRESS_OUTPUT") {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(
            std::path::Path::new(&output).join("PageStress.one"),
            &source,
        )
        .unwrap();
    }
}

#[test]
fn native_changes_on_created_pages_accept_rust_followups() {
    let initial = include_bytes!(
        "../../../corpus/page-lifecycle/creation/native/cold/notebook/Lifecycle.one"
    );
    let store = Store::parse(initial).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut targets = Vec::new();
    for (sid, _) in document.pages().unwrap() {
        let space = &document.spaces[&sid];
        let view = &space.revisions[&space.contexts[&ExGuid::default()]];
        let mut title = None;
        let mut body = None;
        for (id, node) in &view.nodes {
            if let Kind::RichText { text, .. } = &node.kind {
                if text == "New 🦋 é" {
                    title = Some(*id);
                }
                if text.starts_with("Native body ") {
                    body = Some(*id);
                }
            }
        }
        if let (Some(title), Some(body)) = (title, body) {
            targets.push((sid, title, body));
        }
    }
    assert_eq!(targets.len(), 2);
    let mut source = initial.to_vec();
    for (sid, title, body) in targets {
        for (object, prefix) in [(body, "Rust + "), (title, "Reviewed ")] {
            source = ops::page_op(&source, sid, PageOp::Text { text: object, range: 0..0, with: prefix.into() })
                .unwrap()
                .as_bytes()
                .to_vec();
        }
        let store = Store::parse(&source).unwrap();
        let index = RevisionIndex::parse(&store).unwrap();
        let document = Document::parse(&index).unwrap();
        let view = &document.spaces[&sid];
        let view = &view.revisions[&view.contexts[&ExGuid::default()]];
        assert!(
            matches!(&view.nodes[&title].kind, Kind::RichText { text, .. } if text == "Reviewed New 🦋 é")
        );
        assert!(
            matches!(&view.nodes[&body].kind, Kind::RichText { text, .. } if text.starts_with("Rust + Native body "))
        );
        assert!(
            matches!(&view.nodes[&view.roots[&2]].kind, Kind::Metadata { title: Some(title), .. } if title == "Reviewed New 🦋 é")
        );
    }
    if let Some(output) = std::env::var_os("ONESTORE_PAGE_FOLLOWUP_OUTPUT") {
        std::fs::create_dir(&output).unwrap();
        std::fs::write(std::path::Path::new(&output).join("Lifecycle.one"), &source).unwrap();
    }
}

#[test]
fn page_creation_preserves_native_features_and_file_data() {
    for source in [
        include_bytes!("../../../corpus/native-ink/20260905-ui/notebook/synthetic.one").as_slice(),
        include_bytes!("../../../corpus/native-external-assets/notebook/synthetic.one").as_slice(),
        include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one").as_slice(),
    ] {
        let before = Store::parse(source).unwrap();
        let old = RevisionIndex::parse(&before).unwrap();
        let original = Document::parse(&old).unwrap();
        let intent = PageCreation::new(None, Some("Preserved features"), "Author").unwrap();
        let prepared = ops::section_op(source, SectionOp::Create(intent.clone())).unwrap();
        let after = Store::parse(prepared.as_bytes()).unwrap();
        let new = RevisionIndex::parse(&after).unwrap();
        let document = Document::parse(&new).unwrap();
        assert_eq!(
            document.pages().unwrap().len(),
            original.pages().unwrap().len() + 1
        );
        for (sid, space) in &old.spaces {
            for rid in space.revisions.keys() {
                let original = old.resolve(*sid, *rid).unwrap();
                let retained = new.resolve(*sid, *rid).unwrap();
                assert_eq!(format!("{original:?}"), format!("{retained:?}"));
                for object in original.objects.values() {
                    if let Some(onestore::FileDataReference::Internal(id)) =
                        object.file_reference().unwrap()
                    {
                        assert_eq!(before.file_data(id).unwrap(), after.file_data(id).unwrap());
                    }
                }
            }
            if *sid != old.root {
                assert_eq!(space.labels, new.spaces[sid].labels);
            }
        }
    }
}

/// A page created into an empty section as OneNote 2010 creates one (its title's date and
/// time fields, then a colour) reads back with both; a page kept from elsewhere keeps its
/// identity and creation time.
#[test]
fn dated_pages_in_an_empty_section_read_back_with_their_date_and_colour() {
    let source = onestore::create_empty_section("New Section 1.one", Some(0x00e4a88a)).unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    assert!(Document::parse(&index).unwrap().pages().unwrap().is_empty());
    let creation = PageCreation::new(None, Some(""), "Rust Author")
        .unwrap()
        .dated("Sunday, September 27, 2026", "2:23 AM")
        .unwrap();
    let retained: PageCreation =
        serde_json::from_slice(&serde_json::to_vec(&creation).unwrap()).unwrap();
    assert_eq!(retained, creation);
    let space = creation.space();
    let identity = [7; 16];
    let kept = PageCreation::new(None, Some("Kept"), "Rust Author")
        .unwrap()
        .keeping(identity, 133_700_000_001_234_567)
        .unwrap();
    let image = ops::edited(
        &source,
        vec![
            onestore::op::Op::Section(SectionOp::Create(creation.clone())),
            onestore::op::Op::Page {
                space,
                op: PageOp::Color(Some(0x00f2f9d4)),
            },
            onestore::op::Op::Section(SectionOp::Create(kept.clone())),
        ],
    )
    .unwrap();
    let store = Store::parse(&image).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    let page = Page::from_space(&document, space).unwrap();
    assert_eq!(page.color, Some(0x00f2f9d4));
    assert_eq!(page.created, Some(creation.created()));
    let [PageObject::Title(title)] = &page.objects[..] else {
        panic!("{:?}", page.objects)
    };
    let shown: Vec<_> = title.outlines[1]
        .paragraphs
        .iter()
        .map(|paragraph| {
            let text = paragraph.text().unwrap();
            (text.date_field, text.text.text().to_owned())
        })
        .collect();
    assert_eq!(title.date, Some(title.outlines[1].id));
    assert_eq!(
        shown,
        [
            // OneNote's own new pages read the same: the fields' order names their roles.
            (None, "Sunday, September 27, 2026".to_owned()),
            (None, "2:23 AM".to_owned()),
        ]
    );
    let kept_page = Page::from_space(&document, kept.space()).unwrap();
    assert_eq!(kept_page.identity, Some(identity));
    assert_eq!(kept_page.created, Some(133_700_000_001_234_567));
    assert_eq!(kept_page.color, None);
    // Clearing the colour stores OneNote's "No color".
    let cleared = ops::page_edited(&image, space, vec![PageOp::Color(None)]).unwrap();
    let store = Store::parse(&cleared).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    assert_eq!(Page::from_space(&document, space).unwrap().color, None);
}
