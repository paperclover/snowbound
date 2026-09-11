use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Kind},
    page::{Page, PageObject, Paragraph},
};

const SOURCE: &[u8] = include_bytes!("../../../corpus/paragraph-format/before/synthetic.one");

fn page(bytes: &[u8]) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let page = Page::from_space(&document, space).unwrap();
            (page.title == "Paragraph formatting").then_some((space, page))
        })
        .unwrap()
}

#[test]
fn paragraph_properties_preserve_native_mixed_runs_and_shared_styles() {
    let (space, before) = page(SOURCE);
    let mut after = before.clone();
    let mut count = 0;
    for object in &mut after.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                let text = paragraph.text_mut().unwrap();
                let mut start = 0;
                text.text = Paragraph::from_runs(text.text.spans().iter().map(|span| {
                    let value = text.text.text()[start..span.end].to_owned();
                    start = span.end;
                    let mut format = span.format.clone();
                    format.alignment = Some(1);
                    format.space_before = Some(0.0);
                    format.space_after = Some(2.0);
                    format.line_spacing = Some(16.0);
                    (value, format)
                }));
                count += 1;
            }
        }
    }
    assert_eq!(count, 6);
    let prepared = PreparedEdit::page(SOURCE, space, &after, "Format author").unwrap();
    assert_eq!(page(prepared.as_bytes()).1, after);
    let before_store = Store::parse(SOURCE).unwrap();
    let before_index = RevisionIndex::parse(&before_store).unwrap();
    let before_doc = Document::parse(&before_index).unwrap();
    let after_store = Store::parse(prepared.as_bytes()).unwrap();
    let after_index = RevisionIndex::parse(&after_store).unwrap();
    let old = before_index.resolve_active(space).unwrap();
    let new = after_index.resolve_active(space).unwrap();
    for (id, node) in &before_doc.active(space).unwrap().nodes {
        if matches!(node.kind, Kind::Style { .. }) {
            assert_eq!(old.objects[id].data, new.objects[id].data);
        }
    }
    if let Some(directory) = std::env::var_os("ONESTORE_PARAGRAPH_FORMAT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), prepared.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", after_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn paragraph_formatting_rejects_invalid_values_and_conflicting_run_settings() {
    let (space, before) = page(SOURCE);
    let invalid: &[fn(&mut onestore::document::Format)] = &[
        |format| format.alignment = Some(3),
        |format| format.space_before = Some(-1.0),
        |format| format.space_after = Some(f32::NAN),
        |format| format.line_spacing = Some(f32::INFINITY),
        |format| format.space_after = Some(1_000_100.0),
    ];
    for change in invalid {
        let mut after = before.clone();
        let text = after
            .objects
            .iter_mut()
            .find_map(|object| match object {
                PageObject::Outline(outline) => {
                    outline.paragraphs.iter_mut().find_map(|p| p.text_mut())
                }
                _ => None,
            })
            .unwrap();
        let mut format = text.text.format_at(0).unwrap().clone();
        change(&mut format);
        text.text = Paragraph::new(text.text.text().to_owned(), format);
        assert!(PreparedEdit::page(SOURCE, space, &after, "Author").is_err());
    }
    let mut after = before.clone();
    let text = after
        .objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) => {
                outline.paragraphs.iter_mut().find_map(|p| p.text_mut())
            }
            _ => None,
        })
        .unwrap();
    let mut left = text.text.format_at(0).unwrap().clone();
    let mut right = left.clone();
    left.alignment = Some(1);
    right.alignment = Some(2);
    text.text = Paragraph::from_runs([("Left".into(), left), ("Right".into(), right)]);
    assert!(PreparedEdit::page(SOURCE, space, &after, "Author").is_err());
}
