use onestore::{
    RevisionIndex, Store,
    document::Document,
    page::{Page, PageObject, ParagraphContent},
};
use std::{collections::BTreeSet, fs, path::Path};

fn sections(directory: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if !matches!(
                path.file_name().and_then(|n| n.to_str()),
                Some("private" | "recovered")
            ) {
                sections(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "one") {
            out.push(path);
        }
    }
}

fn identities(page: &Page) -> Vec<onestore::ExGuid> {
    fn paragraphs(list: &[onestore::page::PageParagraph], ids: &mut Vec<onestore::ExGuid>) {
        for paragraph in list {
            ids.push(paragraph.id);
            match &paragraph.content {
                ParagraphContent::Text(text) => ids.push(text.id),
                ParagraphContent::Ink(ink) => ids.push(ink.id),
                ParagraphContent::Table(table) => {
                    ids.push(table.id);
                    for row in &table.rows {
                        ids.push(row.id);
                        for cell in &row.cells {
                            ids.push(cell.id);
                            paragraphs(&cell.paragraphs, ids);
                            ids.extend(cell.unsupported.iter().map(|u| u.id));
                        }
                    }
                }
                ParagraphContent::Image(image) => ids.push(image.id),
                ParagraphContent::Attachment(attachment) => ids.push(attachment.id),
                ParagraphContent::Unsupported(unsupported) => ids.push(unsupported.id),
            }
        }
    }
    let mut ids = Vec::new();
    for object in &page.objects {
        ids.push(object.id());
        match object {
            PageObject::Outline(outline) => {
                paragraphs(&outline.paragraphs, &mut ids);
                ids.extend(outline.unsupported.iter().map(|u| u.id));
            }
            PageObject::Title(title) => {
                for outline in &title.outlines {
                    ids.push(outline.id);
                    paragraphs(&outline.paragraphs, &mut ids);
                    ids.extend(outline.unsupported.iter().map(|u| u.id));
                }
            }
            PageObject::Image(_)
            | PageObject::Attachment(_)
            | PageObject::Ink(_)
            | PageObject::Unsupported(_) => {}
        }
    }
    ids
}

#[test]
fn every_corpus_page_builds_a_model_with_distinct_identities() {
    let mut files = Vec::new();
    sections(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus")),
        &mut files,
    );
    files.sort();
    assert!(files.len() > 300, "{} sections", files.len());
    let mut pages = 0;
    let mut failures = Vec::new();
    for path in &files {
        let bytes = fs::read(path).unwrap();
        let Ok(store) = Store::parse(&bytes) else {
            continue;
        };
        let Ok(index) = RevisionIndex::parse(&store) else {
            continue;
        };
        let Ok(document) = Document::parse(&index) else {
            continue;
        };
        let Ok(listed) = document.pages() else {
            continue;
        };
        for (space, _) in listed {
            pages += 1;
            match Page::from_space(&document, space) {
                Ok(page) => {
                    let ids = identities(&page);
                    let distinct: BTreeSet<_> = ids.iter().collect();
                    if distinct.len() != ids.len() {
                        failures.push(format!("{}: repeated identity", path.display()));
                    }
                }
                Err(error) => failures.push(format!("{}: {}", path.display(), error.message)),
            }
        }
    }
    assert!(pages > 1000, "{pages} pages");
    assert!(
        failures.is_empty(),
        "{}\n{} failures",
        failures.join("\n"),
        failures.len()
    );
}
