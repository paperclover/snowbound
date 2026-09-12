use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Format, Kind},
    page::{
        Definition, Page, PageObject, PageParagraph, ParagraphContent, TextObject, text::new_id,
    },
};
use std::collections::BTreeMap;

const TREES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one");
const AUTHOR: &str = "List author";

fn page_by_title(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
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

fn list_ids(page: &Page) -> Vec<Vec<ExGuid>> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) if !outline.title => {
                Some(outline.paragraphs.iter().map(|p| p.lists.clone()).collect())
            }
            _ => None,
        })
        .next()
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

fn bullet_definition() -> Definition {
    Definition {
        kind: Kind::List {
            font: Some("Courier New".into()),
            format: Some("\u{25cb}".into()),
            restart: None,
            bullet: Some(4),
        },
        format: Format {
            font_size: Some(11.0),
            color: Some(0xff000000),
            ..Default::default()
        },
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
fn a_numbered_paragraph_becomes_plain_while_its_neighbours_keep_their_nodes() {
    let (space, before) = page_by_title(TREES, "Move numbered subtree down");
    let lists = list_ids(&before);
    let numbered: Vec<usize> = (0..lists.len()).filter(|i| !lists[*i].is_empty()).collect();
    assert!(numbered.len() >= 2, "{lists:?}");
    let mut after = before.clone();
    body_paragraphs(&mut after)[numbered[0]].lists.clear();
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    expected.definitions.remove(&lists[numbered[0]][0]);
    assert_eq!(stored, expected);
    for other in &numbered[1..] {
        for node in &lists[*other] {
            assert_eq!(
                raw_object(written.as_bytes(), space, *node),
                raw_object(TREES, space, *node)
            );
        }
    }
    assert_eq!(
        PreparedEdit::page(written.as_bytes(), space, &stored, AUTHOR)
            .unwrap()
            .as_bytes(),
        written.as_bytes()
    );
}

#[test]
fn bullets_and_numbering_write_from_definitions_and_read_back() {
    let (space, before) = page_by_title(TREES, "Move numbered subtree down");
    let lists = list_ids(&before);
    let numbered = (0..lists.len()).find(|i| !lists[*i].is_empty()).unwrap();
    let plain = (0..lists.len()).find(|i| lists[*i].is_empty()).unwrap();
    let mut after = before.clone();
    let bullet = new_id().unwrap();
    after.definitions.insert(bullet, bullet_definition());
    let template = body_paragraphs(&mut after)[plain].clone();
    body_paragraphs(&mut after)[plain].lists = vec![bullet];
    let number = new_id().unwrap();
    let mut numbering = after.definitions[&lists[numbered][0]].clone();
    let Kind::List { restart, .. } = &mut numbering.kind else {
        panic!()
    };
    *restart = Some(3);
    after.definitions.insert(number, numbering);
    let mut fresh = plain_paragraph(&template, "Numbered from three");
    fresh.lists = vec![number];
    body_paragraphs(&mut after).push(fresh);
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    assert_eq!(
        PreparedEdit::page(written.as_bytes(), space, &stored, AUTHOR)
            .unwrap()
            .as_bytes(),
        written.as_bytes()
    );
}

#[test]
fn a_list_definition_changes_in_place() {
    let (space, before) = page_by_title(TREES, "Move numbered subtree down");
    let lists = list_ids(&before);
    let numbered = (0..lists.len()).find(|i| !lists[*i].is_empty()).unwrap();
    let node = lists[numbered][0];
    let mut after = before.clone();
    let definition = after.definitions.get_mut(&node).unwrap();
    let Kind::List { restart, .. } = &mut definition.kind else {
        panic!()
    };
    *restart = Some(7);
    definition.format.bold = Some(true);
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    assert_eq!(list_ids(&stored)[numbered], vec![node]);
}

#[test]
fn a_definition_shared_between_paragraphs_becomes_a_node_per_paragraph() {
    let (space, before) = page_by_title(TREES, "Move numbered subtree down");
    let lists = list_ids(&before);
    let numbered = (0..lists.len()).find(|i| !lists[*i].is_empty()).unwrap();
    let plain = (0..lists.len()).find(|i| lists[*i].is_empty()).unwrap();
    let mut shared = before.clone();
    body_paragraphs(&mut shared)[plain].lists = lists[numbered].clone();
    let written = PreparedEdit::page(TREES, space, &shared, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let after = list_ids(&stored);
    assert_eq!(after[numbered], lists[numbered]);
    assert_eq!(after[plain].len(), 1);
    assert_ne!(after[plain], lists[numbered]);
    assert_eq!(
        stored.definitions[&after[plain][0]],
        before.definitions[&lists[numbered][0]]
    );
}

#[test]
fn missing_and_foreign_list_definitions_are_refused() {
    let (space, before) = page_by_title(TREES, "Move numbered subtree down");
    let lists = list_ids(&before);
    let plain = (0..lists.len()).find(|i| lists[*i].is_empty()).unwrap();
    let mut missing = before.clone();
    body_paragraphs(&mut missing)[plain].lists = vec![new_id().unwrap()];
    assert!(PreparedEdit::page(TREES, space, &missing, AUTHOR).is_err());
    let mut foreign = before.clone();
    let paragraph = body_paragraphs(&mut foreign)[plain].id;
    foreign.definitions.insert(paragraph, bullet_definition());
    body_paragraphs(&mut foreign)[plain].lists = vec![paragraph];
    assert!(PreparedEdit::page(TREES, space, &foreign, AUTHOR).is_err());
    let mut style: BTreeMap<ExGuid, Definition> = before.definitions.clone();
    if let Some((id, definition)) = style
        .iter_mut()
        .find(|(_, d)| matches!(d.kind, Kind::Style { .. }))
    {
        definition.format.bold = Some(true);
        let mut edited = before.clone();
        edited.definitions.insert(*id, definition.clone());
        assert!(PreparedEdit::page(TREES, space, &edited, AUTHOR).is_err());
    }
}

/// A fresh page gains a bullet, two numbered paragraphs with a nested numbered child, a
/// numbering restart and a trailing plain paragraph; `ONESTORE_LIST_EXPORT` names a new
/// directory receiving the candidate for a cold native reopen.
#[test]
fn bullets_numbering_nesting_and_restarts_publish_on_a_fresh_page() {
    let source = onestore::create_section("lists.one", "Bullet item", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let before = Page::from_space(&document, space).unwrap();
    let (_, trees) = page_by_title(TREES, "Move numbered subtree down");
    let numbering = trees
        .definitions
        .values()
        .find(|d| {
            matches!(
                d.kind,
                Kind::List {
                    format: Some(_),
                    ..
                }
            )
        })
        .unwrap()
        .clone();
    let scenario = std::env::var("ONESTORE_LIST_SCENARIO").unwrap_or_else(|_| "all".into());
    let wants = |name: &str| scenario == "all" || scenario == name;
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    if wants("bullet") {
        let bullet = new_id().unwrap();
        after.definitions.insert(bullet, bullet_definition());
        body_paragraphs(&mut after)[0].lists = vec![bullet];
    }
    for (text, level, restart, name) in [
        ("First numbered", 1, None, "numbered"),
        ("Second numbered", 1, None, "numbered"),
        ("Nested numbered", 2, None, "all"),
        ("Restarted at three", 1, Some(3), "restart"),
        ("Nested plain", 2, None, "nested"),
    ] {
        if !wants(name) {
            continue;
        }
        let mut paragraph = plain_paragraph(&template, text);
        if name != "nested" {
            let id = new_id().unwrap();
            let mut definition = numbering.clone();
            let Kind::List { restart: value, .. } = &mut definition.kind else {
                panic!()
            };
            *value = restart;
            after.definitions.insert(id, definition);
            paragraph.lists = vec![id];
        }
        paragraph.level = level;
        if level == 2 {
            paragraph.parent = Some(body_paragraphs(&mut after).last().unwrap().id);
        }
        body_paragraphs(&mut after).push(paragraph);
    }
    body_paragraphs(&mut after).push(plain_paragraph(&template, "Plain again"));
    let written = PreparedEdit::page(&source, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let mut expected = after.clone();
    expected.title = stored.title.clone();
    assert_eq!(stored, expected);
    if let Some(directory) = std::env::var_os("ONESTORE_LIST_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("lists.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("lists.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
