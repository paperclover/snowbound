//! Paragraph tree edits expressed as page-model saves: moves, deletions, their
//! reconciliation against native remote images, and emptied table cells.

use super::*;
use model_schedule::move_subtree;
use onestore::page::{Paragraph, ParagraphContent, TableCell};

/// Saves an edited model of `space`, reaching content that outline helpers cannot.
fn save_page(cache: &Replica, space: ExGuid, edit: impl FnOnce(&mut Page)) -> Option<u64> {
    let source = cache.snapshot().unwrap();
    let mut page = page_of(&source, space);
    edit(&mut page);
    cache
        .save(&source, space, &page, model_ops::AUTHOR)
        .unwrap()
}

#[test]
fn twelve_offline_clients_reconcile_tree_text_and_interrupted_publication() {
    let mut random = 1940_u64;
    for _ in 0..24 {
        let input: Vec<_> = (0..256)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                random as u8
            })
            .collect();
        model_schedule::run(&input);
    }
}

/// The one-outline fixture with three further sibling paragraphs.
fn fixture() -> (Vec<u8>, ExGuid, ExGuid, [ExGuid; 4], [ExGuid; 4]) {
    let (source, space, outline, text) = super::fixture();
    let mut page = page_of(&source, space);
    let mut anchor = text;
    for at in 1..4 {
        anchor = model_ops::insert_after(&mut page, anchor, &format!("Sibling {at}")).1;
    }
    let source = PreparedEdit::page(&source, space, &page, "Author")
        .unwrap()
        .as_bytes()
        .to_vec();
    let stored = page_of(&source, space);
    let siblings = &outline_of(&stored, outline).paragraphs;
    assert_eq!(siblings.len(), 4);
    let ids = std::array::from_fn(|at| siblings[at].id);
    let texts = std::array::from_fn(|at| siblings[at].text().unwrap().id);
    (source, space, outline, ids, texts)
}

/// Paragraph identities of an outline in model order.
fn order(bytes: &[u8], space: ExGuid, outline: ExGuid) -> Vec<ExGuid> {
    outline_of(&page_of(bytes, space), outline)
        .paragraphs
        .iter()
        .map(|paragraph| paragraph.id)
        .collect()
}

/// Adds a plain paragraph as the last child of `parent`.
fn insert_child(page: &mut Page, parent: ExGuid, value: &str) {
    for outline in outlines_mut(page) {
        let Some(at) = outline.paragraphs.iter().position(|p| p.id == parent) else {
            continue;
        };
        let mut child = model_ops::fresh_paragraph(&outline.paragraphs[at], value);
        child.parent = Some(parent);
        child.level = outline.paragraphs[at].level + 1;
        let level = outline.paragraphs[at].level;
        let mut end = at + 1;
        while end < outline.paragraphs.len() && outline.paragraphs[end].level > level {
            end += 1;
        }
        outline.paragraphs.insert(end, child);
        return;
    }
    panic!("the parent paragraph is on the page");
}

#[test]
fn move_preserves_remote_content_and_dependent_edits_through_reopen() {
    let (source, space, outline, paragraphs, texts) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::create(&path, &source).unwrap();
    let id = save(&cache, texts[0], |page| {
        move_subtree(page, paragraphs[0], None, None);
        replace_text(page, texts[0], 0..0, "Local ");
    })
    .unwrap()
    .unwrap();
    let local = cache.snapshot().unwrap();
    let queue = cache.pending().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert_eq!(cache.pending().unwrap(), queue);
    assert_eq!(cache.snapshot().unwrap(), local);
    let mut server = Server::new(&remote_with(&source, space, |page| {
        insert_child(page, paragraphs[0], "New remote child");
        restyle(page, texts[1], 0..7, |format| format.bold = Some(true));
    }));
    published(&cache, &mut server, id);
    let durable = page_of(&server.durable, space);
    let published = outline_of(&durable, outline);
    assert_eq!(
        published
            .paragraphs
            .iter()
            .map(|p| p.id)
            .collect::<Vec<_>>()[..4],
        [paragraphs[1], paragraphs[2], paragraphs[3], paragraphs[0]]
    );
    let child = &published.paragraphs[4];
    assert_eq!(child.parent, Some(paragraphs[0]));
    assert_eq!(child.text().unwrap().text.text(), "New remote child");
    assert_eq!(text_of(&durable, texts[0]), "Local Original 🦀 é");
    let sibling = &paragraph_with(&durable, texts[1])
        .unwrap()
        .text()
        .unwrap()
        .text;
    assert_eq!(sibling.spans()[0].format.bold, Some(true));
    assert!(cache.pending().unwrap().is_empty());
    assert_eq!(server.publications, 1);
}

#[test]
fn native_empty_child_list_normalization_merges_with_a_local_deletion() {
    let source = include_bytes!("../../../../corpus/outline-edit/empty-children/before.one");
    let native = include_bytes!("../../../../corpus/outline-edit/empty-children/remote.one");
    let intent: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../corpus/outline-edit/empty-children/intent.json"
    ))
    .unwrap();
    let space: ExGuid = intent[0].as_str().unwrap().parse().unwrap();
    let object: ExGuid = intent[1]["object"].as_str().unwrap().parse().unwrap();
    let page = page_of(source, space);
    let outline = body_outlines(&page)
        .into_iter()
        .find(|outline| outline.paragraphs.iter().any(|p| p.id == object))
        .unwrap();
    let deleted = outline
        .paragraphs
        .iter()
        .find(|p| p.id == object)
        .unwrap()
        .text()
        .unwrap()
        .id;
    let left = outline.paragraphs[0].text().unwrap().id;
    let outline = outline.id;
    let concurrent: Vec<String> = body_outlines(&page_of(native, space))
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter())
        .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
        .collect();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("cache.sqlite");
    let cache = Replica::create(&path, source).unwrap();
    let change = |page: &mut Page| {
        delete_paragraph(page, deleted);
        replace_text(page, left, 0..0, "Local ");
    };
    let id = save(&cache, left, change).unwrap().unwrap();
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    let mut server = Server::new(native);
    assert!(matches!(
        cache.sync_once(&mut server).unwrap(),
        Some((published, EditStatus::Published { .. })) if published == id
    ));
    assert_eq!(server.publications, 1);
    assert!(cache.pending().unwrap().is_empty());
    let durable = page_of(&server.durable, space);
    assert!(!order(&server.durable, space, outline).contains(&object));
    assert!(text_of(&durable, left).starts_with("Local "));
    let published: Vec<String> = body_outlines(&durable)
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter())
        .filter_map(|paragraph| Some(paragraph.text()?.text.text().to_owned()))
        .collect();
    for text in concurrent {
        assert!(
            text == "🐈" || published.iter().any(|value| value.ends_with(&text)),
            "the merge keeps concurrent remote content: {text}"
        );
    }
}

#[test]
fn deletion_requires_review_of_remote_content_and_preserves_later_work() {
    let (source, space, outline, paragraphs, texts) = fixture();
    let mut page = page_of(&source, space);
    insert_child(&mut page, paragraphs[0], "Descendant");
    let source = PreparedEdit::page(&source, space, &page, "Author")
        .unwrap()
        .as_bytes()
        .to_vec();
    let child = outline_of(&page_of(&source, space), outline).paragraphs[1]
        .text()
        .unwrap()
        .id;
    let remotes: [Change<Page>; 4] = [
        Box::new(move |page| replace_text(page, texts[0], 0..0, "Remote ")),
        Box::new(move |page| replace_text(page, child, 0..0, "Remote ")),
        Box::new(move |page| {
            restyle(page, child, 0..10, |format| format.italic = Some(true));
        }),
        Box::new(move |page| insert_child(page, paragraphs[0], "New child")),
    ];
    for remote in remotes {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cache.sqlite");
        let cache = Replica::create(&path, &source).unwrap();
        let change = |page: &mut Page| {
            delete_paragraph(page, texts[0]);
            replace_text(page, texts[1], 0..0, "Local ");
        };
        let id = save(&cache, texts[1], change).unwrap().unwrap();
        let queue = cache.pending().unwrap();
        let local = cache.snapshot().unwrap();
        let mut server = Server::new(&remote_with(&source, space, remote));
        assert_eq!(
            cache.sync_once(&mut server).unwrap(),
            Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
        );
        assert_eq!(server.publications, 0);
        assert_eq!(cache.pending().unwrap(), queue);
        assert_eq!(cache.snapshot().unwrap(), local);
        let observed = cache.remote_snapshot().unwrap();
        assert!(
            cache
                .review_page(id, &source, &observed, &page_of(&observed, space))
                .is_err()
        );
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(
            cache.status(id).unwrap(),
            Some(EditStatus::Conflict(ConflictKind::ContentChanged))
        );
        review(&cache, id, texts[1], change).unwrap();
        published(&cache, &mut server, id);
        assert_eq!(
            order(&server.durable, space, outline),
            paragraphs[1..].to_vec()
        );
        assert_eq!(
            text_of(&page_of(&server.durable, space), texts[1]),
            "Local Sibling 1"
        );
    }
}

#[test]
fn sibling_and_ancestry_changes_have_explicit_merge_or_conflict() {
    let (source, space, outline, paragraphs, texts) = fixture();
    // The published order of the fixture's paragraphs, or a conflict.
    let cases: [(Change<Page>, Option<&[usize]>); 5] = [
        (
            Box::new(move |page| {
                model_ops::insert_after(page, texts[0], "New sibling");
            }),
            Some(&[1, 2, 3, 0]),
        ),
        (
            Box::new(move |page| delete_paragraph(page, texts[1])),
            Some(&[2, 3, 0]),
        ),
        (
            Box::new(move |page| move_subtree(page, paragraphs[1], None, None)),
            Some(&[2, 3, 0, 1]),
        ),
        (
            Box::new(move |page| move_subtree(page, paragraphs[0], Some(paragraphs[1]), None)),
            None,
        ),
        (Box::new(move |page| delete_paragraph(page, texts[0])), None),
    ];
    for (remote, expected) in cases {
        let directory = tempfile::tempdir().unwrap();
        let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
        let id = save(&cache, texts[0], |page| {
            move_subtree(page, paragraphs[0], None, None)
        })
        .unwrap()
        .unwrap();
        let mut server = Server::new(&remote_with(&source, space, remote));
        let result = cache.sync_once(&mut server).unwrap().unwrap();
        assert_eq!(result.0, id);
        let Some(expected) = expected else {
            assert_eq!(result.1, EditStatus::Conflict(ConflictKind::ContentChanged));
            assert_eq!(server.publications, 0);
            continue;
        };
        assert!(matches!(result.1, EditStatus::Published { .. }));
        let published: Vec<ExGuid> = order(&server.durable, space, outline)
            .into_iter()
            .filter(|id| paragraphs.contains(id))
            .collect();
        let expected: Vec<ExGuid> = expected.iter().map(|at| paragraphs[*at]).collect();
        assert_eq!(published, expected);
        assert_eq!(
            text_of(&page_of(&server.durable, space), texts[2]),
            "Sibling 2"
        );
    }
}

#[test]
fn independently_satisfied_move_confirms_without_republication() {
    let (source, space, _, paragraphs, texts) = fixture();
    let directory = tempfile::tempdir().unwrap();
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source).unwrap();
    let id = save(&cache, texts[0], |page| {
        move_subtree(page, paragraphs[0], None, None)
    })
    .unwrap()
    .unwrap();
    let mut server = Server::new(&remote_with(&source, space, |page| {
        move_subtree(page, paragraphs[0], None, None);
    }));
    published(&cache, &mut server, id);
    assert_eq!((server.publications, server.confirmations), (0, 1));
}

fn cells(page: &Page) -> Vec<&TableCell> {
    body_outlines(page)
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter())
        .filter_map(|paragraph| match &paragraph.content {
            ParagraphContent::Table(table) => Some(table),
            _ => None,
        })
        .flat_map(|table| table.rows.iter())
        .flat_map(|row| row.cells.iter())
        .collect()
}

fn cell_of(page: &Page, id: ExGuid) -> &TableCell {
    cells(page)
        .into_iter()
        .find(|cell| cell.id == id)
        .expect("the cell is on the page")
}

fn cell_mut(page: &mut Page, id: ExGuid) -> &mut TableCell {
    outlines_mut(page)
        .into_iter()
        .flat_map(|outline| outline.paragraphs.iter_mut())
        .filter_map(|paragraph| match &mut paragraph.content {
            ParagraphContent::Table(table) => Some(table),
            _ => None,
        })
        .flat_map(|table| table.rows.iter_mut())
        .flat_map(|row| row.cells.iter_mut())
        .find(|cell| cell.id == id)
        .expect("the cell is on the page")
}

/// Moves a cell's sole paragraph to the end of another cell, or deletes it.
fn empty_cell(page: &mut Page, cell: ExGuid, destination: Option<ExGuid>) {
    let moved = cell_mut(page, cell).paragraphs.remove(0);
    let Some(destination) = destination else {
        return;
    };
    let destination = cell_mut(page, destination);
    let mut moved = moved;
    moved.level = destination.paragraphs[0].level;
    moved.parent = None;
    destination.paragraphs.push(moved);
}

#[test]
fn emptied_cell_replacement_is_durable_and_cannot_be_silently_omitted_on_replay() {
    let source =
        include_bytes!("../../../../corpus/outline-edit/tree/before/notebook/synthetic.one");
    let store = Store::parse(source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let (space, page) = document
        .pages()
        .unwrap()
        .into_iter()
        .find_map(|(space, _)| {
            let page = Page::from_space(&document, space).unwrap();
            (page.title == "Delete sole cell paragraph").then_some((space, page))
        })
        .unwrap();
    let other = document
        .pages()
        .unwrap()
        .into_iter()
        .find(|(other, _)| *other != space)
        .unwrap()
        .0;
    let text = |cell: &TableCell| cell.paragraphs[0].text().unwrap().id;
    let cell = cells(&page)
        .into_iter()
        .find(|cell| {
            cell.paragraphs[0]
                .text()
                .unwrap()
                .text
                .text()
                .starts_with("Target ")
        })
        .unwrap();
    let (cell, target) = (cell.id, text(cell));
    let neighbour = cells(&page)
        .into_iter()
        .find(|other| other.id != cell)
        .unwrap();
    let (neighbour, neighbour_text) = (neighbour.id, text(neighbour));
    for move_out in [false, true] {
        for competing in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let path = directory.path().join("cell.sqlite");
            let cache = Replica::create(&path, source).unwrap();
            let id = save_page(&cache, space, |page| {
                empty_cell(page, cell, move_out.then_some(neighbour));
            })
            .unwrap();
            let local = cache.snapshot().unwrap();
            let queue = cache.pending().unwrap();
            {
                let page = page_of(&local, space);
                let paragraphs = &cell_of(&page, cell).paragraphs;
                assert_eq!(paragraphs.len(), 1);
                assert_ne!(paragraphs[0].text().unwrap().id, target);
                assert!(paragraphs[0].text().unwrap().text.text().is_empty());
            }
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            assert_eq!(cache.pending().unwrap(), queue);
            assert_eq!(cache.snapshot().unwrap(), local);
            let mut server = Server::new(&if competing {
                remote_with(source, space, |page| {
                    let target = cell_mut(page, cell);
                    let mut sibling =
                        model_ops::fresh_paragraph(&target.paragraphs[0], "Remote sibling");
                    sibling.level = target.paragraphs[0].level;
                    target.paragraphs.push(sibling);
                })
            } else {
                remote_with(source, other, |page| {
                    let text = body_outlines(page)[0].paragraphs[0].text().unwrap().id;
                    replace_text(page, text, 0..0, "Remote ");
                })
            });
            if competing {
                assert_eq!(
                    cache.sync_once(&mut server).unwrap(),
                    Some((id, EditStatus::Conflict(ConflictKind::ContentChanged)))
                );
                assert_eq!(server.publications, 0);
                assert_eq!(cache.pending().unwrap(), queue);
                assert_eq!(cache.snapshot().unwrap(), local);
                continue;
            }
            published(&cache, &mut server, id);
            let replacement = {
                let durable = page_of(&server.durable, space);
                let paragraphs = &cell_of(&durable, cell).paragraphs;
                assert_eq!(paragraphs.len(), 1);
                assert_ne!(paragraphs[0].text().unwrap().id, target);
                paragraphs[0].text().unwrap().id
            };
            let edit = save_page(&cache, space, |page| {
                let cell = cell_mut(page, cell);
                let text = cell.paragraphs[0].text_mut().unwrap();
                text.text = Paragraph::new(
                    "Local replacement".into(),
                    text.text.format_at(0).unwrap().clone(),
                );
            })
            .unwrap();
            drop(cache);
            let cache = Replica::open(&path).unwrap();
            published(&cache, &mut server, edit);
            let durable = page_of(&server.durable, space);
            let paragraphs = &cell_of(&durable, cell).paragraphs;
            assert_eq!(paragraphs.len(), 1);
            assert_eq!(paragraphs[0].text().unwrap().id, replacement);
            assert_eq!(
                paragraphs[0].text().unwrap().text.text(),
                "Local replacement"
            );
            let published = cell_of(&durable, neighbour);
            assert_eq!(
                published.paragraphs.len(),
                1 + usize::from(move_out),
                "the moved paragraph joins its destination"
            );
            assert_eq!(published.paragraphs[0].text().unwrap().id, neighbour_text);
            if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_TREE_OUTPUT") {
                let output = std::path::PathBuf::from(output)
                    .join(if move_out { "cell-move" } else { "cell-delete" })
                    .join("candidate");
                std::fs::create_dir_all(&output).unwrap();
                std::fs::write(output.join("synthetic.one"), &server.durable).unwrap();
            }
        }
    }
}

/// The move or deletion each native fixture page is reconciled against.
fn native_change(
    name: &str,
    outline: ExGuid,
    target: (ExGuid, ExGuid),
    anchor: Option<ExGuid>,
) -> Change<Page> {
    let name = name.to_owned();
    Box::new(move |page| {
        let present = paragraph_with(page, target.1).is_some();
        match name.as_str() {
            "Move leaf down" | "Move subtree down" if present => {
                move_subtree(page, target.0, None, None);
            }
            "Move subtree up" if present => {
                move_subtree(page, target.0, None, anchor);
            }
            "Delete outline" | "Move outline" | "Resize outline" | "Automatic outline size" => {
                page.objects
                    .retain(|object| !matches!(object, PageObject::Outline(o) if o.id == outline));
            }
            _ if present => {
                delete_paragraph(page, target.1);
                // An outline cannot survive without a paragraph.
                page.objects.retain(
                    |object| !matches!(object, PageObject::Outline(o) if o.paragraphs.is_empty()),
                );
            }
            _ => {}
        }
    })
}

#[test]
fn native_tree_and_layout_changes_reconcile_without_discarding_unreviewed_content() {
    let mut server = Server::new(NATIVE);
    let mut records = Vec::new();
    for (name, space, outline, target, dependent) in native_pages(BEFORE) {
        let page = page_of(BEFORE, space);
        let paragraphs = &outline_of(&page, outline).paragraphs;
        let target = (
            paragraphs
                .iter()
                .find(|p| p.text().is_some_and(|text| text.id == target))
                .unwrap()
                .id,
            target,
        );
        let anchor = paragraphs
            .iter()
            .find(|p| {
                p.text()
                    .is_some_and(|text| text.text.text().starts_with("Anchor"))
            })
            .map(|paragraph| paragraph.id);
        let change = native_change(&name, outline, target, anchor);
        let save_both = |page: &mut Page| {
            change(page);
            replace_text(page, dependent, 0..0, "Offline ");
        };
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("native.sqlite");
        let cache = Replica::create(&path, BEFORE).unwrap();
        let id = save(&cache, dependent, save_both).unwrap().unwrap();
        let local = cache.snapshot().unwrap();
        let queue = cache.pending().unwrap();
        let merged = match cache.sync_once(&mut server).unwrap().unwrap() {
            (actual, EditStatus::Published { .. }) => {
                assert_eq!(actual, id, "{name}");
                true
            }
            result => {
                assert_eq!(
                    result,
                    (id, EditStatus::Conflict(ConflictKind::ContentChanged)),
                    "{name}"
                );
                assert_eq!(cache.pending().unwrap(), queue);
                assert_eq!(cache.snapshot().unwrap(), local);
                drop(cache);
                let cache = Replica::open(&path).unwrap();
                review(&cache, id, dependent, save_both).unwrap();
                published(&cache, &mut server, id);
                false
            }
        };
        let durable = page_of(&server.durable, space);
        assert!(
            text_of(&durable, dependent).starts_with("Offline "),
            "{name} keeps its dependent text"
        );
        let mut reapplied = durable.clone();
        change(&mut reapplied);
        assert_eq!(reapplied, durable, "{name} keeps its reconciled change");
        records.push((name, if merged { "merged" } else { "reviewed" }));
    }
    let reviewed = records
        .iter()
        .filter(|(_, result)| *result == "reviewed")
        .count();
    assert_eq!((records.len(), reviewed), (14, 7), "{records:?}");
    if let Some(output) = std::env::var_os("ONESTORE_OFFLINE_TREE_OUTPUT") {
        let output = std::path::PathBuf::from(output);
        std::fs::create_dir_all(output.join("candidate")).unwrap();
        std::fs::write(output.join("candidate/synthetic.one"), &server.durable).unwrap();
        std::fs::write(
            output.join("cases.json"),
            serde_json::to_vec_pretty(&records).unwrap(),
        )
        .unwrap();
    }
}
