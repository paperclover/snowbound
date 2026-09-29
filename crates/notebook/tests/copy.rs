use notebook::session::{Event, Section};
use onestore::{
    RevisionIndex, Store,
    document::Document,
    page::{Page, PageObject},
};
use std::{
    collections::BTreeMap,
    io::Write,
    path::Path,
    time::{Duration, Instant},
};

/// Pages the writer authored for other rows: text with an attachment and its icon, files on
/// the page, a table, nested tables with cell subtrees, an inserted picture, a page-level ink
/// drawing, tags, lists, equations and paragraph formatting.
const SOURCES: &[&str] = &[
    "attachment-edit/icon/candidate/files.one",
    "attachment-floating/candidate/files.one",
    "table-edit/created/candidate/tables.one",
    "table-edit/nested/candidate/synthetic.one",
    "picture-edit/inserted/candidate/pictures.one",
    "ink-edit/drawing/candidate/ink.one",
    "tag-edit/candidate/tags.one",
    "list-edit/candidate/lists.one",
    "math-edit/written/candidate/math.one",
    "paragraph-format/candidate/synthetic.one",
];

fn pages(bytes: &[u8]) -> Vec<Page> {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| Page::from_space(&document, space).unwrap())
        .collect()
}

/// Everything but identities and the title object: what a copy must preserve. Identities
/// are numbered by first appearance in the content, so definitions compare by use.
fn shape(page: &Page) -> String {
    let mut page = page.clone();
    flatten(&mut page);
    let mut value = serde_json::to_value(&page).unwrap();
    let mut names: BTreeMap<String, String> = BTreeMap::new();
    fn name(names: &mut BTreeMap<String, String>, text: &str) -> String {
        let next = format!("id{}", names.len());
        names.entry(text.to_owned()).or_insert(next).clone()
    }
    fn scrub(value: &mut serde_json::Value, names: &mut BTreeMap<String, String>) {
        match value {
            serde_json::Value::String(text) if text.parse::<onestore::ExGuid>().is_ok() => {
                *text = name(names, text);
            }
            serde_json::Value::Array(items) => {
                for item in items {
                    scrub(item, names);
                }
            }
            serde_json::Value::Object(fields) => {
                // The writer's canonical formatting: an unset flag is false and unset
                // spacing is zero.
                fields.retain(|key, value| {
                    // OneNote lays outlines out again on open.
                    key != "max_height"
                        && !value.is_null()
                        && *value != serde_json::Value::Bool(false)
                        && value.as_f64() != Some(0.0)
                });
                let keys: Vec<String> = fields.keys().cloned().collect();
                for key in keys {
                    let mut item = fields.remove(&key).unwrap();
                    scrub(&mut item, names);
                    let key = if key.parse::<onestore::ExGuid>().is_ok() {
                        name(names, &key)
                    } else {
                        key
                    };
                    fields.insert(key, item);
                }
            }
            _ => {}
        }
    }
    let object = value.as_object_mut().unwrap();
    object.remove("identity");
    object.remove("created");
    object.remove("margin_origin");
    let objects = object["objects"].as_array_mut().unwrap();
    objects.retain(|object| object.get("Title").is_none());
    scrub(&mut object["objects"], &mut names);
    // The destination page brings its own title style.
    object["definitions"]
        .as_object_mut()
        .unwrap()
        .retain(|_, definition| definition["kind"]["type"] != "Style");
    scrub(&mut object["definitions"], &mut names);
    value.to_string()
}

/// Moves paragraph-level formatting into each span, where the reader resolves it: the
/// writer keeps a copied style's formatting on the paragraph or the runs as it sees fit.
fn flatten(page: &mut Page) {
    fn paragraphs(list: &mut [onestore::page::PageParagraph]) {
        for paragraph in list {
            let format = std::mem::take(&mut paragraph.format);
            match &mut paragraph.content {
                onestore::page::ParagraphContent::Text(text) => {
                    let mut at = 0;
                    let runs: Vec<(String, onestore::document::Format)> = text
                        .text
                        .spans()
                        .iter()
                        .map(|span| {
                            let piece = text.text.text()[at..span.end].to_owned();
                            at = span.end;
                            (piece, span.format.inherit(&format))
                        })
                        .collect();
                    text.text = onestore::page::Paragraph::from_runs(runs);
                }
                onestore::page::ParagraphContent::Table(table) => {
                    for row in &mut table.rows {
                        for cell in &mut row.cells {
                            paragraphs(&mut cell.paragraphs);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for object in &mut page.objects {
        if let PageObject::Outline(outline) = object {
            paragraphs(&mut outline.paragraphs);
        }
    }
}

/// `NOTEBOOK_COPY_EXPORT` names a new directory receiving the destination section for a
/// cold reopen.
#[test]
fn pages_copy_into_another_section_with_their_content_and_fresh_identities() {
    let temporary = tempfile::tempdir().unwrap();
    let file = temporary.path().join("copies.one");
    let cache = temporary.path().join("cache.sqlite");
    std::fs::write(
        &file,
        onestore::create_section("copies.one", "Destination", "Author").unwrap(),
    )
    .unwrap();
    let section = Section::open(&file, &cache, || {}).unwrap();
    let mut expected = Vec::new();
    let mut grouped = Vec::new();
    for source in SOURCES {
        let bytes = std::fs::read(
            Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus")).join(source),
        )
        .unwrap();
        for page in pages(&bytes) {
            if page
                .objects
                .iter()
                .any(|object| matches!(object, PageObject::Unsupported(_)))
            {
                continue;
            }
            let copy = match page.copy() {
                Ok(copy) => copy,
                Err(error) if error.message == "Outline groups cannot be copied" => {
                    grouped.push(page.title.clone());
                    continue;
                }
                Err(error) => panic!("{source} {}: {error}", page.title),
            };
            assert_ne!(copy.identity, page.identity);
            let space = section
                .import_page(&page, "Copier")
                .unwrap_or_else(|e| panic!("{source} {}: {e}", page.title));
            expected.push((space, page.title.clone(), shape(&copy)));
        }
    }
    assert!(expected.len() >= 8, "{} pages", expected.len());
    // Outdenting leaves paragraphs at levels only an outline group carries.
    assert_eq!(
        grouped,
        [
            "Outdent first group",
            "Delete only grouped subtree",
            "Delete unindented sibling after group",
            "Outdent child across indentation gap"
        ]
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    while !section.pending().unwrap().is_empty() {
        for event in section.events() {
            if let Event::Failed(message) = event {
                panic!("{message}");
            }
        }
        assert!(
            Instant::now() < deadline,
            "publication did not finish: {:?}",
            section.pending().unwrap()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let stored = std::fs::read(&file).unwrap();
    let written = pages(&stored);
    assert_eq!(written.len(), expected.len() + 1);
    for ((space, title, expected), page) in expected.iter().zip(&written[1..]) {
        let _ = space;
        assert_eq!(&page.title, title);
        let stored = shape(page);
        if &stored != expected {
            let dump = std::env::temp_dir().join("copy-mismatch");
            std::fs::create_dir_all(&dump).unwrap();
            std::fs::write(dump.join("expected.json"), expected).unwrap();
            std::fs::write(dump.join("stored.json"), &stored).unwrap();
            panic!("{title}: copy differs, see {}", dump.display());
        }
    }
    section.delete_pages(&[expected.last().unwrap().0]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(60);
    while !section.pending().unwrap().is_empty() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(pages(&std::fs::read(&file).unwrap()).len(), expected.len());
    section.close().unwrap();
    if let Some(directory) = std::env::var_os("NOTEBOOK_COPY_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir_all(&directory).unwrap();
        let bytes = std::fs::read(&file).unwrap();
        std::fs::File::create_new(directory.join("copies.one"))
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        let file_id = Store::parse(&bytes).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("copies.one", file_id)])
                .unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join("expected-count.txt"),
            expected.len().to_string(),
        )
        .unwrap();
    }
}
