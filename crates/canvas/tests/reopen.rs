#![cfg(feature = "gpu")]

use canvas::{
    gpu::{Paper, page::PageScene},
    layout::TextEngine,
};
use onestore::{
    Arena, ExGuid, PageCreation, RevisionIndex, Section, Store,
    document::Document,
    op::{Edit, Op, SectionOp},
    page::{Page, PageObject, ParagraphContent},
};
use std::{fs, path::Path};

fn pages(section: &[u8]) -> Vec<Page> {
    let store = Store::parse(section).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    spaces(&document)
        .into_iter()
        .map(|space| Page::from_space(&document, space).unwrap())
        .collect()
}

fn spaces(document: &Document<'_>) -> Vec<ExGuid> {
    let mut spaces: Vec<_> = document
        .pages()
        .unwrap()
        .into_iter()
        .map(|(space, _)| space)
        .collect();
    spaces.dedup();
    spaces
}

/// What `notebook::session::Section::import_page` applies: a new titled page holding a copy of
/// the editor's body objects.
#[test]
fn an_imported_editor_page_reopens_with_its_title_editable() {
    let section = onestore::create_section("reopen.one", "Body 🦀 é", "Author").unwrap();
    let mut engine = TextEngine::default();
    let (_, editor) = PageScene::from_page(pages(&section).remove(0), &mut engine).unwrap();
    let copy = editor.page().unwrap().copy().unwrap();
    let creation = PageCreation::new(None, Some("Imported 🦋"), "Author").unwrap();
    let arena = Arena::default();
    let mut imported = Section::open(&arena, section).unwrap();
    let import = SectionOp::Import {
        creation,
        page: copy,
    };
    let edit = Edit {
        at: 134_000_000_000_000_000,
        ops: vec![Op::Section(import)],
    };
    imported.apply("Author", &edit).unwrap();
    imported.seal().unwrap();

    let reopened = pages(&imported.image()).pop().unwrap();
    let (_, editor) = PageScene::from_page(reopened.clone(), &mut engine).unwrap();
    assert_eq!(editor.page().unwrap(), reopened);
    let texts = |title: bool| -> Vec<String> {
        editor
            .outlines()
            .iter()
            .filter(|outline| outline.title == title)
            .map(|outline| {
                outline
                    .document()
                    .paragraphs()
                    .next()
                    .unwrap()
                    .text()
                    .to_string()
            })
            .collect()
    };
    assert_eq!(texts(true), ["Imported 🦋"]);
    assert_eq!(texts(false), ["Body 🦀 é"]);
}

#[test]
fn every_corpus_page_opens() {
    fn sections(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sections(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "one") {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    sections(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus"),
        &mut found,
    );
    let mut engine = TextEngine::default();
    let (mut pages, mut failures) = (0, Vec::new());
    for path in found {
        let bytes = fs::read(&path).unwrap();
        // Malformed and encrypted sections are the reader's tests' concern.
        let Ok(store) = Store::parse(&bytes) else {
            continue;
        };
        let Ok(index) = RevisionIndex::parse(&store) else {
            continue;
        };
        let Ok(document) = Document::parse(&index) else {
            continue;
        };
        for space in spaces(&document) {
            pages += 1;
            let result = Page::from_space(&document, space)
                .map_err(|error| format!("{error:?}"))
                .and_then(|page| {
                    PageScene::from_page(page, &mut engine).map_err(|error| format!("{error:?}"))
                });
            if let Err(error) = result {
                failures.push(format!("{} {space}: {error}", path.display()));
            }
        }
    }
    assert!(pages > 2000, "{pages}");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A page opens whatever its pictures hold, and saving keeps their stored data.
#[test]
fn undecodable_pictures_show_placeholders_and_keep_their_data() {
    let corpus = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let section = |path: &str| pages(&fs::read(corpus.join(path)).unwrap());
    let mut engine = TextEngine::default();

    // A PNG whose checksum does not match, inside an outline.
    let page = section("writer/complex-01/notebook/synthetic.one")
        .into_iter()
        .find(|page| page.title.starts_with("Fictitious"))
        .unwrap();
    let picture = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => {
                outline
                    .paragraphs
                    .iter()
                    .find_map(|paragraph| match &paragraph.content {
                        ParagraphContent::Image(image) => Some(image.id),
                        _ => None,
                    })
            }
            _ => None,
        })
        .unwrap();
    let (mut scene, editor) = PageScene::from_page(page.clone(), &mut engine).unwrap();
    scene.settle(Some(&editor), 1.0, Paper::WHITE);
    assert!(scene.image(picture).is_none());
    assert_eq!(editor.page().unwrap(), page);

    // A TIFF on the page renders; with its data damaged it becomes the placeholder.
    let mut page = section("media-edit/candidate/Features.one")
        .into_iter()
        .find(|page| page.title == "Image tiff")
        .unwrap();
    let (mut scene, editor) = PageScene::from_page(page.clone(), &mut engine).unwrap();
    scene.settle(Some(&editor), 1.0, Paper::WHITE);
    let PageObject::Image(tiff) = page
        .objects
        .iter_mut()
        .find(|object| matches!(object, PageObject::Image(_)))
        .unwrap()
    else {
        unreachable!()
    };
    assert!(scene.image(tiff.id).is_some());
    tiff.bytes = Some(b"not a picture".as_slice().into());
    let (scene, editor) = PageScene::from_page(page.clone(), &mut engine).unwrap();
    assert_eq!(
        scene
            .read_only(Some(&editor))
            .map(|object| object.message)
            .collect::<Vec<_>>(),
        ["Image unavailable\nRead-only"]
    );
    assert_eq!(editor.page().unwrap(), page);
}
