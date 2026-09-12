use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Format, Kind, Tag},
    page::{
        Definition, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id,
    },
};

const TREES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one");
const AUTHOR: &str = "Tag author";

fn page_by_title(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let page = Page::from_space(&document, space).unwrap();
            (page.title == title).then_some((space, page))
        })
        .unwrap()
}

fn page_in(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    Page::from_space(&Document::parse(&index).unwrap(), space).unwrap()
}

fn body_paragraphs(page: &mut Page) -> &mut Vec<PageParagraph> {
    page.objects
        .iter_mut()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(&mut outline.paragraphs),
            _ => None,
        })
        .unwrap()
}

/// Tag positions in the element's extra arena are assigned on read; equality ignores them.
fn normalized(mut page: Page) -> Page {
    for object in &mut page.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                for tag in &mut paragraph.tags {
                    tag.extra_set = 0;
                }
                if let Some(text) = paragraph.text_mut() {
                    for tag in &mut text.tags {
                        tag.extra_set = 0;
                    }
                }
            }
        }
    }
    page
}

fn assert_same(written: &[u8], space: ExGuid, expected: &Page) -> Page {
    let stored = page_in(written, space);
    let mut expected = expected.clone();
    expected.title = stored.title.clone();
    assert_eq!(normalized(stored.clone()), normalized(expected));
    stored
}

fn tagged(page: &Page) -> usize {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => Some(
                outline
                    .paragraphs
                    .iter()
                    .position(|p| p.text().is_some_and(|t| !t.tags.is_empty()))
                    .unwrap(),
            ),
            _ => None,
        })
        .unwrap()
}

fn raw_object(bytes: &[u8], space: ExGuid, id: ExGuid) -> String {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    format!(
        "{:?}",
        index.resolve_active(space).unwrap().objects[&id].data
    )
}

fn definition(label: &str, shape: u16) -> Definition {
    Definition {
        kind: Kind::TagDefinition {
            label: Some(label.into()),
            action_type: Some(0),
            shape: Some(shape),
            color: None,
            highlight: None,
        },
        format: Format::default(),
    }
}

fn tag(definition: ExGuid, completed: bool) -> Tag {
    Tag {
        definition: Some(definition),
        action_type: None,
        status: u16::from(completed),
        created: Some(1_262_401_445),
        completed: completed.then_some(1_262_405_106),
        start: None,
        due: None,
        task_id: None,
        extra_set: 0,
    }
}

fn plain_paragraph(template: &PageParagraph, text: &str) -> PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.content = ParagraphContent::Text(TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new(
            text.into(),
            Format {
                font: Some("Calibri".into()),
                font_size: Some(11.0),
                language: Some(1033),
                ..Default::default()
            },
        ),
        tags: Vec::new(),
    });
    paragraph
}

#[test]
fn removing_a_tag_leaves_its_definition_and_the_other_paragraphs_untouched() {
    let (space, before) = page_by_title(TREES, "Indent first subtree");
    let at = tagged(&before);
    let mut after = before.clone();
    let definition = body_paragraphs(&mut after)[at].text().unwrap().tags[0]
        .definition
        .unwrap();
    body_paragraphs(&mut after)[at]
        .text_mut()
        .unwrap()
        .tags
        .clear();
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let mut expected = after.clone();
    expected.definitions.remove(&definition);
    let stored = assert_same(written.as_bytes(), space, &expected);
    assert!(!stored.definitions.contains_key(&definition));
    assert_eq!(
        raw_object(written.as_bytes(), space, definition),
        raw_object(TREES, space, definition)
    );
    let count = body_paragraphs(&mut after).len();
    let other = body_paragraphs(&mut after)[(at + 1) % count]
        .text()
        .unwrap()
        .id;
    assert_eq!(
        raw_object(written.as_bytes(), space, other),
        raw_object(TREES, space, other)
    );
}

#[test]
fn an_existing_definition_tags_another_paragraph_and_a_task_completes_in_place() {
    let (space, before) = page_by_title(TREES, "Indent first subtree");
    let at = tagged(&before);
    let mut after = before.clone();
    let existing = body_paragraphs(&mut after)[at].text().unwrap().tags[0].clone();
    let definition = existing.definition.unwrap();
    let count = body_paragraphs(&mut after).len();
    let other = (at + 1) % count;
    body_paragraphs(&mut after)[other]
        .text_mut()
        .unwrap()
        .tags
        .push(tag(definition, false));
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = assert_same(written.as_bytes(), space, &after);
    let mut completed = stored.clone();
    let text = body_paragraphs(&mut completed)[other].text_mut().unwrap();
    text.tags[0].status = 1;
    text.tags[0].completed = Some(1_262_405_106);
    let again = PreparedEdit::page(written.as_bytes(), space, &completed, AUTHOR).unwrap();
    assert_same(again.as_bytes(), space, &completed);
    assert_eq!(
        PreparedEdit::page(
            again.as_bytes(),
            space,
            &page_in(again.as_bytes(), space),
            AUTHOR
        )
        .unwrap()
        .as_bytes(),
        again.as_bytes()
    );
}

#[test]
fn tags_without_a_known_definition_are_refused() {
    let (space, before) = page_by_title(TREES, "Indent first subtree");
    let mut missing = before.clone();
    body_paragraphs(&mut missing)[0]
        .text_mut()
        .unwrap()
        .tags
        .push(tag(new_id().unwrap(), false));
    assert!(PreparedEdit::page(TREES, space, &missing, AUTHOR).is_err());
    let mut repeated = before.clone();
    let at = tagged(&repeated);
    let existing = body_paragraphs(&mut repeated)[at].text().unwrap().tags[0].clone();
    body_paragraphs(&mut repeated)[at]
        .text_mut()
        .unwrap()
        .tags
        .push(tag(existing.definition.unwrap(), false));
    assert!(PreparedEdit::page(TREES, space, &repeated, AUTHOR).is_err());
    let mut wrong = before.clone();
    let list = wrong
        .definitions
        .iter()
        .find(|(_, d)| matches!(d.kind, Kind::List { .. }))
        .map(|(id, _)| *id);
    if let Some(list) = list {
        body_paragraphs(&mut wrong)[0]
            .text_mut()
            .unwrap()
            .tags
            .push(tag(list, false));
        assert!(PreparedEdit::page(TREES, space, &wrong, AUTHOR).is_err());
    }
}

/// A fresh page gains two tag definitions and three tagged paragraphs, one completed;
/// `ONESTORE_TAG_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn new_definitions_and_tags_publish_on_a_fresh_page() {
    let source = onestore::create_section("tags.one", "Open task", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let before = Page::from_space(&document, space).unwrap();
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let task = new_id().unwrap();
    let important = new_id().unwrap();
    after.definitions.insert(task, definition("Rust task", 3));
    let mut important_definition = definition("Important", 13);
    let Kind::TagDefinition {
        action_type,
        color,
        highlight,
        ..
    } = &mut important_definition.kind
    else {
        panic!()
    };
    *action_type = Some(1);
    *color = Some(0x0000_00ff);
    *highlight = Some(0x0000_ffff);
    after.definitions.insert(important, important_definition);
    body_paragraphs(&mut after)[0]
        .text_mut()
        .unwrap()
        .tags
        .push(tag(task, false));
    let mut done = plain_paragraph(&template, "Completed task");
    done.text_mut().unwrap().tags.push(tag(task, true));
    body_paragraphs(&mut after).push(done);
    let mut both = plain_paragraph(&template, "Important open task");
    both.text_mut().unwrap().tags.push(tag(important, false));
    both.text_mut().unwrap().tags.push(tag(task, false));
    body_paragraphs(&mut after).push(both);
    body_paragraphs(&mut after).push(plain_paragraph(&template, "Untagged"));
    let written = PreparedEdit::page(&source, space, &after, AUTHOR).unwrap();
    assert_same(written.as_bytes(), space, &after);
    if let Some(directory) = std::env::var_os("ONESTORE_TAG_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("tags.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("tags.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
