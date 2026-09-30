//! Pictures as OneNote 2010 stores them in the wild.

use canvas::{editor::CanvasEditor, layout::TextEngine};
use onestore::{
    Arena, Section,
    op::{Edit, Op},
    page::{Page, PageObject, ParagraphContent},
};

fn page(bytes: &[u8], title: &str) -> Page {
    let arena = Arena::default();
    let mut section = Section::open(&arena, bytes.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == title)
        .unwrap();
    section.page(space).unwrap()
}

/// A picture COM put on the page stores no layout size, only its own (PictureWidth and
/// PictureHeight), and shows at that size (`corpus/print`).
#[test]
fn a_page_picture_without_a_layout_size_shows_at_its_own() {
    let page = page(
        include_bytes!("../../../corpus/print/native/Print.one"),
        "Printing test",
    );
    let [picture] = &page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Image(image) => Some(image),
            _ => None,
        })
        .collect::<Vec<_>>()[..]
    else {
        panic!("one picture");
    };
    assert_eq!(picture.layout.max_width, None);
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    let origin = [picture.layout.x.unwrap(), picture.layout.y.unwrap()];
    assert_eq!(
        editor.image_placement(picture.id),
        Some((origin, [150.0, 90.0]))
    );
    #[cfg(feature = "gpu")]
    {
        let (scene, editor) = canvas::gpu::page::PageScene::from_page(page, &mut engine).unwrap();
        assert_eq!(scene.read_only(Some(&editor)).count(), 0);
    }
}

const OBJECTS: &[u8] = include_bytes!("../../../corpus/object-tags/native/notebook/files.one");

/// The picture or file `id` holds, wherever it is on the page.
fn tags_of(page: &Page, id: onestore::ExGuid) -> Vec<onestore::document::Tag> {
    let contents = page.objects.iter().flat_map(|object| match object {
        PageObject::Outline(outline) => outline
            .paragraphs
            .iter()
            .map(|paragraph| match &paragraph.content {
                ParagraphContent::Image(image) => (image.id, image.tags.clone()),
                ParagraphContent::Attachment(file) => (file.id, file.tags.clone()),
                _ => (paragraph.id, Vec::new()),
            })
            .collect(),
        PageObject::Image(image) => vec![(image.id, image.tags.clone())],
        PageObject::Attachment(file) => vec![(file.id, file.tags.clone())],
        _ => Vec::new(),
    });
    contents
        .filter(|(object, _)| *object == id)
        .map(|(_, tags)| tags)
        .next()
        .unwrap()
}

/// OneNote 2010 stores a tag given to a picture or file on the object itself, in an
/// outline or on the page, and draws it in a column to its left centred on it; a click on a
/// check box checks it, one revision each, and undo clears it again
/// (`corpus/object-tags`). `CANVAS_OBJECT_TAGS_EXPORT` names a new directory receiving the
/// candidate for a cold reopen.
#[test]
fn tags_on_pictures_and_files_draw_and_check() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, OBJECTS.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Pictures and files")
        .unwrap();
    let page = section.page(space).unwrap();
    let outline = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if outline.paragraphs.len() == 4 => Some(outline),
            _ => None,
        })
        .unwrap();
    let [(picture_paragraph, picture), (_, file)] = [1, 2].map(|index| {
        let paragraph = &outline.paragraphs[index];
        let id = match &paragraph.content {
            ParagraphContent::Image(image) => image.id,
            ParagraphContent::Attachment(file) => file.id,
            _ => panic!("a picture and a file"),
        };
        (paragraph.id, id)
    });
    let outline_id = outline.id;
    let (page_picture, page_file) = (
        page.objects
            .iter()
            .find_map(|object| match object {
                PageObject::Image(image) if !image.tags.is_empty() => Some(image.id),
                _ => None,
            })
            .unwrap(),
        page.objects
            .iter()
            .find_map(|object| match object {
                PageObject::Attachment(file) => Some(file.id),
                _ => None,
            })
            .unwrap(),
    );
    for id in [picture, file, page_picture, page_file] {
        let tags = tags_of(&page, id);
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].status & 1, 0);
        assert!(page.definitions.contains_key(&tags[0].definition.unwrap()));
    }

    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page.clone(), &mut engine).unwrap();
    // The outline's tags stand beside the picture and the file they mark.
    let shaped = editor
        .outlines()
        .iter()
        .find(|item| item.id == outline_id)
        .unwrap()
        .shaped();
    let marked: Vec<_> = shaped.tags().map(|(paragraph, ..)| paragraph).collect();
    assert_eq!(marked, [picture_paragraph, outline.paragraphs[2].id]);
    #[cfg(feature = "gpu")]
    {
        // The page picture at (400, 86), 120 by 90, has its tag 24.75 points to its left,
        // centred on it.
        let (scene, shown) =
            canvas::gpu::page::PageScene::from_page(page.clone(), &mut engine).unwrap();
        let hit = |point| scene.hit_test(point, Some(&shown), |_| None::<()>);
        assert_eq!(
            hit([376.0, 126.0]),
            Some(canvas::gpu::page::SceneHit::Check(page_picture))
        );
        assert_eq!(
            hit([374.0, 126.0]),
            None,
            "the tag's column starts 24.75 points left"
        );
    }

    let save = |editor: &mut CanvasEditor, section: &mut Section| {
        let ops = editor
            .take_ops()
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space, op })
            .collect();
        section
            .apply(
                "Author",
                &Edit {
                    at: 134_000_000_000_000_000,
                    ops,
                },
            )
            .unwrap();
        section.seal().unwrap().unwrap();
    };
    editor.click_object_check(page_picture).unwrap();
    save(&mut editor, &mut section);
    editor.click_object_check(page_file).unwrap();
    save(&mut editor, &mut section);
    editor
        .click_check(&mut engine, outline_id, picture_paragraph)
        .unwrap();
    save(&mut editor, &mut section);
    let stored = section.page(space).unwrap();
    for (id, checked) in [
        (page_picture, true),
        (page_file, true),
        (picture, true),
        (file, false),
    ] {
        assert_eq!(tags_of(&stored, id)[0].status & 1 != 0, checked, "{id:?}");
    }
    if let Some(directory) = std::env::var_os("CANVAS_OBJECT_TAGS_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        let written = section.image();
        std::fs::write(directory.join("files.one"), &written).unwrap();
        let file_id = onestore::Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("files.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
    assert!(editor.undo(&mut engine).unwrap());
    assert!(editor.undo(&mut engine).unwrap());
    save(&mut editor, &mut section);
    let stored = section.page(space).unwrap();
    assert_eq!(tags_of(&stored, page_file)[0].status & 1, 0);
    assert_eq!(tags_of(&stored, picture)[0].status & 1, 0);
    assert_eq!(tags_of(&stored, page_picture)[0].status & 1, 1);
}

/// Text OneNote recognised in a picture is found by search, as OneNote finds it
/// (`corpus/object-tags`).
#[test]
fn search_finds_text_recognised_in_pictures() {
    let page = page(OBJECTS, "Pictures and files");
    let recognised: Vec<_> = page
        .objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Image(image) => image.text.as_ref().map(|text| text.text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(recognised, ["GLACIER HARBOR\nInvoice 4471"]);
    let mut index = canvas::search::Index::default();
    index.set(canvas::search::Entry::new(
        "files",
        onestore::ExGuid::default(),
        &page,
        0,
    ));
    let found = index.search(&canvas::search::Query::new("invoice 4471"), |_| true);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].snippet, "Invoice 4471");
    assert!(
        index
            .search(&canvas::search::Query::new("harb"), |_| true)
            .len()
            == 1
    );
}

/// A picture's link reads back from where OneNote stores it (WzHyperlinkUrl), and a linked
/// picture deleted and brought back by undo keeps it (`corpus/picture-link`).
/// `CANVAS_PICTURE_LINK_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn a_linked_picture_keeps_its_link_through_undo() {
    let source = include_bytes!("../../../corpus/m6/native-features-01/notebook/Features.one");
    let arena = Arena::default();
    let mut section = Section::open(&arena, source.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Image png")
        .unwrap();
    let page = section.page(space).unwrap();
    let linked = |page: &Page| -> Vec<(onestore::ExGuid, Option<String>)> {
        page.objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Image(image) => Some((image.id, image.link.clone())),
                _ => None,
            })
            .collect()
    };
    let before = linked(&page);
    let [(id, Some(link))] = &before[..] else {
        panic!("one linked picture");
    };
    assert_eq!(link, "https://example.invalid/image/png");
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    editor.remove_image(&mut engine, *id).unwrap();
    assert!(editor.undo(&mut engine).unwrap());
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    assert_eq!(linked(&section.page(space).unwrap()), before);
    if let Some(directory) = std::env::var_os("CANVAS_PICTURE_LINK_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        let written = section.image();
        std::fs::write(directory.join("Features.one"), &written).unwrap();
        let file_id = onestore::Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents(
                "Open Notebook.onetoc2",
                &[("Features.one", file_id)],
            )
            .unwrap(),
        )
        .unwrap();
    }
}

/// A tag command on a selected picture or file tags the object itself, as OneNote 2010
/// does (`corpus/object-tags`): a tag it has comes off, one it lacks goes on unchecked.
#[test]
fn tag_commands_tag_a_selected_picture_or_file() {
    use canvas::editor::{Formatting, NoteTag};
    let arena = Arena::default();
    let mut section = Section::open(&arena, OBJECTS.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Pictures and files")
        .unwrap();
    let page = section.page(space).unwrap();
    let page_picture = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Image(image) if !image.tags.is_empty() => Some(image.id),
            _ => None,
        })
        .unwrap();
    let file = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => {
                outline.paragraphs.iter().find_map(|p| match &p.content {
                    ParagraphContent::Attachment(file) => Some(file.id),
                    _ => None,
                })
            }
            _ => None,
        })
        .unwrap();
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    let to_do = Formatting::Tag(NoteTag::defaults()[0].clone(), 0);
    let important = Formatting::Tag(NoteTag::defaults()[1].clone(), 1);
    assert!(
        editor
            .format_object(&mut engine, page_picture, &to_do)
            .unwrap()
    );
    assert!(editor.format_object(&mut engine, file, &important).unwrap());
    assert!(
        !editor
            .format_object(&mut engine, file, &Formatting::Bullets)
            .unwrap()
    );
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    let stored = section.page(space).unwrap();
    assert!(tags_of(&stored, page_picture).is_empty());
    let tags = tags_of(&stored, file);
    assert_eq!(tags.len(), 2);
    assert!(matches!(
        &stored.definitions[&tags[0].definition.unwrap()].kind,
        onestore::document::Kind::TagDefinition { label: Some(label), .. } if label == "Important"
    ));
}

/// A picture deleted and brought back by undo keeps the text OneNote recognised in it,
/// written back as OneNote stores it (`corpus/object-tags/restored`).
/// `CANVAS_RECOGNIZED_EXPORT` names a new directory receiving the candidate for a cold
/// reopen.
#[test]
fn a_restored_picture_keeps_its_recognised_text() {
    let arena = Arena::default();
    let mut section = Section::open(&arena, OBJECTS.to_vec()).unwrap();
    let (space, ..) = section
        .pages()
        .unwrap()
        .into_iter()
        .find(|page| page.1 == "Pictures and files")
        .unwrap();
    let page = section.page(space).unwrap();
    let recognized = |page: &Page| {
        page.objects
            .iter()
            .find_map(|object| match object {
                PageObject::Image(image) if image.text.is_some() => {
                    Some((image.id, image.text.clone().unwrap()))
                }
                _ => None,
            })
            .unwrap()
    };
    let (id, text) = recognized(&page);
    assert_eq!(text.stored.len(), 2, "its language and word layout");
    let mut engine = TextEngine::default();
    let mut editor = CanvasEditor::from_page(page, &mut engine).unwrap();
    editor.remove_image(&mut engine, id).unwrap();
    assert!(editor.undo(&mut engine).unwrap());
    let ops = editor
        .take_ops()
        .unwrap()
        .into_iter()
        .map(|op| Op::Page { space, op })
        .collect();
    section
        .apply(
            "Author",
            &Edit {
                at: 134_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
    section.seal().unwrap();
    assert_eq!(recognized(&section.page(space).unwrap()), (id, text));
    if let Some(directory) = std::env::var_os("CANVAS_RECOGNIZED_EXPORT") {
        let directory = std::path::PathBuf::from(directory);
        std::fs::create_dir(&directory).unwrap();
        let written = section.image();
        std::fs::write(directory.join("files.one"), &written).unwrap();
        let file_id = onestore::Store::parse(&written).unwrap().header.file_id;
        std::fs::write(
            directory.join("Open Notebook.onetoc2"),
            onestore::create_table_of_contents("Open Notebook.onetoc2", &[("files.one", file_id)])
                .unwrap(),
        )
        .unwrap();
    }
}
