use onestore::{
    ExGuid, PreparedEdit, RevisionIndex, Store,
    document::{Document, Format},
    page::{
        Page, PageObject, PageParagraph, ParagraphContent, Table, TableCell, TableColumn, TableRow,
        TextObject, text::new_id,
    },
};

const TREES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one");
const AUTHOR: &str = "Table author";

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

fn table_mut(page: &mut Page) -> &mut Table {
    body_paragraphs(page)
        .iter_mut()
        .find_map(|p| match &mut p.content {
            ParagraphContent::Table(table) => Some(table),
            _ => None,
        })
        .unwrap()
}

/// Definitions are read back only while a paragraph references them.
fn referenced(page: &Page) -> std::collections::BTreeSet<ExGuid> {
    fn walk(paragraphs: &[PageParagraph], out: &mut std::collections::BTreeSet<ExGuid>) {
        for paragraph in paragraphs {
            out.extend(paragraph.lists.iter().copied());
            out.extend(paragraph.style);
            out.extend(paragraph.tags.iter().filter_map(|t| t.definition));
            match &paragraph.content {
                ParagraphContent::Text(text) => {
                    out.extend(text.tags.iter().filter_map(|t| t.definition));
                }
                ParagraphContent::Table(table) => {
                    for row in &table.rows {
                        for cell in &row.cells {
                            walk(&cell.paragraphs, out);
                        }
                    }
                }
                ParagraphContent::Image(_)
                | ParagraphContent::Attachment(_)
                | ParagraphContent::Ink(_)
                | ParagraphContent::Unsupported(_) => {}
            }
        }
    }
    let mut out = std::collections::BTreeSet::new();
    for object in &page.objects {
        match object {
            PageObject::Outline(outline) => walk(&outline.paragraphs, &mut out),
            PageObject::Title(title) => {
                for outline in &title.outlines {
                    walk(&outline.paragraphs, &mut out);
                }
            }
            _ => {}
        }
    }
    out
}

/// A new cell without its own or a sibling's indentation table takes OneNote's.
fn with_native_indents(table: &mut Table) {
    for cell in table.rows.iter_mut().flat_map(|row| &mut row.cells) {
        cell.indents = vec![18.0, 0.0, 27.0, 27.0];
    }
}

fn assert_same(written: &[u8], space: ExGuid, expected: &Page) -> Page {
    let stored = page_in(written, space);
    let mut expected = expected.clone();
    expected.title = stored.title.clone();
    let live = referenced(&expected);
    expected.definitions.retain(|id, _| live.contains(id));
    assert_eq!(stored, expected);
    stored
}

fn plain_text(text: &str) -> TextObject {
    TextObject {
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
    }
}

fn cell_paragraph(template: &PageParagraph, text: &str) -> PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.content = ParagraphContent::Text(plain_text(text));
    paragraph
}

fn new_cell(template: &TableCell, text: &str) -> TableCell {
    TableCell {
        id: new_id().unwrap(),
        layout: Default::default(),
        indents: template.indents.clone(),
        shading: None,
        paragraphs: vec![cell_paragraph(&template.paragraphs[0], text)],
        unsupported: Vec::new(),
    }
}

#[test]
fn a_row_and_a_column_are_added_to_a_native_table_and_removed_again() {
    let (space, before) = page_by_title(TREES, "Delete cell subtree");
    let mut after = before.clone();
    let table = table_mut(&mut after);
    assert_eq!((table.rows.len(), table.columns.len()), (1, 2));
    let template = table.rows[0].cells[0].clone();
    table.columns.push(TableColumn {
        width: 120.0,
        locked: false,
    });
    table.rows[0]
        .cells
        .push(new_cell(&template, "Third column"));
    table.rows.push(TableRow {
        id: new_id().unwrap(),
        cells: vec![
            new_cell(&template, "Second row"),
            new_cell(&template, "Second row, second column"),
            new_cell(&template, "Second row, third column"),
        ],
    });
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = assert_same(written.as_bytes(), space, &after);
    assert_eq!(
        PreparedEdit::page(written.as_bytes(), space, &stored, AUTHOR)
            .unwrap()
            .as_bytes(),
        written.as_bytes()
    );
    if let Some(directory) = std::env::var_os("ONESTORE_TABLE_EDIT_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut smaller = stored.clone();
    let table = table_mut(&mut smaller);
    table.columns.remove(1);
    for row in &mut table.rows {
        row.cells.remove(1);
    }
    table.rows.remove(0);
    let again = PreparedEdit::page(written.as_bytes(), space, &smaller, AUTHOR).unwrap();
    let mut stored = assert_same(again.as_bytes(), space, &smaller);
    let table = table_mut(&mut stored);
    assert_eq!((table.rows.len(), table.columns.len()), (1, 2));
}

#[test]
fn column_widths_locks_and_borders_change_in_place() {
    let (space, before) = page_by_title(TREES, "Delete cell subtree");
    let mut after = before.clone();
    let table = table_mut(&mut after);
    table.columns[0].width = 300.0;
    table.columns[1].locked = false;
    table.borders = Some(false);
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    assert_same(written.as_bytes(), space, &after);
}

#[test]
fn cell_shading_and_indents_change_in_place() {
    let (space, before) = page_by_title(TREES, "Delete cell subtree");
    let mut after = before.clone();
    let table = table_mut(&mut after);
    let cell = &mut table.rows[0].cells[0];
    assert_eq!(cell.shading, None);
    cell.shading = Some(0x0000ffff);
    let original_indents = cell.indents.clone();
    cell.indents.clear();
    assert!(PreparedEdit::page(TREES, space, &after, AUTHOR).is_err());
    let cell = &mut table_mut(&mut after).rows[0].cells[0];
    cell.indents = vec![18.0, 0.0, 27.0, 27.0];
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = assert_same(written.as_bytes(), space, &after);
    let mut reset = stored.clone();
    let cell = &mut table_mut(&mut reset).rows[0].cells[0];
    cell.shading = None;
    cell.indents = original_indents;
    let again = PreparedEdit::page(written.as_bytes(), space, &reset, AUTHOR).unwrap();
    assert_eq!(assert_same(again.as_bytes(), space, &reset), before);
}

/// `ONESTORE_NESTED_TABLE_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_table_nests_inside_a_cell_and_is_removed_again() {
    let (space, before) = page_by_title(TREES, "Delete cell subtree");
    let mut after = before.clone();
    let template = table_mut(&mut after).rows[0].cells[0].paragraphs[0].clone();
    let cell = |text: &str| TableCell {
        id: new_id().unwrap(),
        layout: Default::default(),
        indents: Vec::new(),
        shading: None,
        paragraphs: vec![cell_paragraph(&template, text)],
        unsupported: Vec::new(),
    };
    let nested = Table {
        id: new_id().unwrap(),
        columns: vec![TableColumn {
            width: 72.0,
            locked: true,
        }],
        rows: vec![
            TableRow {
                id: new_id().unwrap(),
                cells: vec![cell("Inner one")],
            },
            TableRow {
                id: new_id().unwrap(),
                cells: vec![cell("Inner two")],
            },
        ],
        borders: Some(true),
        layout: Default::default(),
        tags: Vec::new(),
    };
    let mut holder = cell_paragraph(&template, "");
    holder.content = ParagraphContent::Table(nested);
    holder.format = Format::default();
    let holder_id = holder.id;
    table_mut(&mut after).rows[0].cells[1]
        .paragraphs
        .push(holder);
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let ParagraphContent::Table(nested) = &mut table_mut(&mut after).rows[0].cells[1]
        .paragraphs
        .last_mut()
        .unwrap()
        .content
    else {
        panic!()
    };
    with_native_indents(nested);
    let stored = assert_same(written.as_bytes(), space, &after);
    if let Some(directory) = std::env::var_os("ONESTORE_NESTED_TABLE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("synthetic.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("synthetic.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
    let mut removed = stored.clone();
    table_mut(&mut removed).rows[0].cells[1]
        .paragraphs
        .retain(|paragraph| paragraph.id != holder_id);
    let again = PreparedEdit::page(written.as_bytes(), space, &removed, AUTHOR).unwrap();
    assert_eq!(assert_same(again.as_bytes(), space, &removed), before);
}

#[test]
fn inconsistent_tables_are_refused() {
    let (space, before) = page_by_title(TREES, "Delete cell subtree");
    let mut ragged = before.clone();
    let template = table_mut(&mut ragged).rows[0].cells[0].clone();
    table_mut(&mut ragged).rows[0]
        .cells
        .push(new_cell(&template, "Extra"));
    assert!(PreparedEdit::page(TREES, space, &ragged, AUTHOR).is_err());
    let mut empty = before.clone();
    table_mut(&mut empty).rows[0].cells[0].paragraphs.clear();
    let written = PreparedEdit::page(TREES, space, &empty, AUTHOR).unwrap();
    let mut stored = page_in(written.as_bytes(), space);
    let cell = &table_mut(&mut stored).rows[0].cells[0];
    assert_eq!(cell.paragraphs.len(), 1);
    assert_eq!(cell.paragraphs[0].text().unwrap().text.text(), "");
    let mut narrow = before.clone();
    table_mut(&mut narrow).columns[0].width = 10.0;
    assert!(PreparedEdit::page(TREES, space, &narrow, AUTHOR).is_err());
    let mut rowless = before.clone();
    table_mut(&mut rowless).rows.clear();
    assert!(PreparedEdit::page(TREES, space, &rowless, AUTHOR).is_err());
}

/// `ONESTORE_TABLE_EXPORT` names a new directory receiving the candidate for a cold reopen.
#[test]
fn a_new_table_is_created_on_a_fresh_page() {
    let source = onestore::create_section("tables.one", "Before the table", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, _) = document.pages().unwrap()[0];
    let before = Page::from_space(&document, space).unwrap();
    let mut after = before.clone();
    let template = body_paragraphs(&mut after)[0].clone();
    let cell = |text: &str| TableCell {
        id: new_id().unwrap(),
        layout: Default::default(),
        indents: Vec::new(),
        shading: None,
        paragraphs: vec![cell_paragraph(&template, text)],
        unsupported: Vec::new(),
    };
    let table = Table {
        id: new_id().unwrap(),
        columns: vec![
            TableColumn {
                width: 144.0,
                locked: true,
            },
            TableColumn {
                width: 216.0,
                locked: true,
            },
        ],
        rows: vec![
            TableRow {
                id: new_id().unwrap(),
                cells: vec![cell("Name"), cell("Value")],
            },
            TableRow {
                id: new_id().unwrap(),
                cells: vec![cell("Rust 🦀"), cell("Written natively")],
            },
        ],
        borders: Some(true),
        layout: Default::default(),
        tags: Vec::new(),
    };
    let mut holder = cell_paragraph(&template, "");
    holder.content = ParagraphContent::Table(table);
    holder.format = Format::default();
    body_paragraphs(&mut after).push(holder);
    body_paragraphs(&mut after).push(cell_paragraph(&template, "After the table"));
    let written = PreparedEdit::page(&source, space, &after, AUTHOR).unwrap();
    with_native_indents(table_mut(&mut after));
    assert_same(written.as_bytes(), space, &after);
    if let Some(directory) = std::env::var_os("ONESTORE_TABLE_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(directory.join("tables.one"), written.as_bytes()).unwrap();
        let written_store = Store::parse(written.as_bytes()).unwrap();
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("tables.one", written_store.header.file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}
