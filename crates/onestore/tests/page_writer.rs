//! The page writer must publish the same graph the typed writers publish for the same edit,
//! in one transaction, and must round-trip the model it was given.

use onestore::{
    ExGuid, Insertion, OutlineEdit, ParagraphJoin, ParagraphSplit, PreparedEdit, RevisionIndex,
    Store, TextAttribute, TreeEdit,
    document::{Document, Element, Kind, Revision},
    page::{Page, PageObject, ParagraphContent, text::new_id},
};
use std::collections::BTreeSet;

const AUTHOR: &str = "Page author";
const PARAGRAPHS: &[u8] =
    include_bytes!("../../../corpus/paragraph-edit/before/notebook/synthetic.one");
const OUTLINES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");
const TREES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/tree/before/notebook/synthetic.one");

fn page_by_title(bytes: &[u8], title: &str) -> (ExGuid, Page) {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let mut found = None;
    for (space, _) in document.pages().unwrap() {
        let page = Page::from_space(&document, space).unwrap();
        if page.title == title {
            assert!(found.is_none(), "duplicate title {title}");
            found = Some((space, page));
        }
    }
    found.unwrap_or_else(|| panic!("no page titled {title}"))
}

fn page_in(bytes: &[u8], space: ExGuid) -> Page {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    Page::from_space(&document, space).unwrap()
}

fn page_id(bytes: &[u8], space: ExGuid) -> ExGuid {
    let store = Store::parse(bytes).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    document.pages_in(space).unwrap()[0]
}

/// The body outlines in stacking order (not the title's outlines).
fn body(page: &Page) -> Vec<&onestore::page::Outline> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .collect()
}

fn text_ids(outline: &onestore::page::Outline) -> Vec<(ExGuid, ExGuid, String)> {
    outline
        .paragraphs
        .iter()
        .filter_map(|p| p.text().map(|t| (p.id, t.id, t.text.text().to_owned())))
        .collect()
}

fn author_name<'a>(revision: &'a Revision<'_>, id: Option<ExGuid>) -> Option<&'a str> {
    match &revision.nodes.get(&id?)?.kind {
        Kind::Author { name } => name.as_deref(),
        _ => None,
    }
}

fn close(a: Option<u32>, b: Option<u32>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a.abs_diff(b) <= 5,
        (None, None) => true,
        _ => false,
    }
}

fn same_element(revision_a: &Revision<'_>, revision_b: &Revision<'_>, id: ExGuid) {
    let a: &Element = &revision_a.nodes[&id];
    let b: &Element = &revision_b.nodes[&id];
    assert_eq!(a.jcid, b.jcid, "{id}");
    assert_eq!(a.children, b.children, "children of {id}");
    assert_eq!(a.content, b.content, "content of {id}");
    assert_eq!(a.structure, b.structure, "structure of {id}");
    assert_eq!(a.spaces, b.spaces, "spaces of {id}");
    assert_eq!(a.child_level, b.child_level, "child level of {id}");
    assert_eq!(a.layout, b.layout, "layout of {id}");
    assert_eq!(a.format, b.format, "format of {id}");
    assert_eq!(a.tags, b.tags, "tags of {id}");
    assert_eq!(a.media_ids, b.media_ids, "media of {id}");
    assert!(
        close(a.created, b.created),
        "created {id}: {:?} {:?}",
        a.created,
        b.created
    );
    assert_eq!(
        author_name(revision_a, a.original_author),
        author_name(revision_b, b.original_author),
        "original author of {id}"
    );
    // A reorder can be attributed to either sibling, so attribution may differ by this edit's author.
    let latest_a = author_name(revision_a, a.latest_author);
    let latest_b = author_name(revision_b, b.latest_author);
    assert!(
        latest_a == latest_b || latest_a == Some(AUTHOR) || latest_b == Some(AUTHOR),
        "latest author of {id}: {latest_a:?} {latest_b:?}"
    );
    assert_eq!(a.extra, b.extra, "retained fields of {id}");
    match (&a.kind, &b.kind) {
        (
            Kind::RichText {
                text: ta,
                boilerplate: ba,
                paragraph_style: pa,
                ..
            },
            Kind::RichText {
                text: tb,
                boilerplate: bb,
                paragraph_style: pb,
                ..
            },
        ) => {
            assert_eq!(ta, tb, "text of {id}");
            assert_eq!((ba, pa), (bb, pb), "text style of {id}");
            let runs_a = revision_a.text_runs(id).unwrap();
            let runs_b = revision_b.text_runs(id).unwrap();
            assert_eq!(runs_a.len(), runs_b.len(), "run count of {id}");
            for (ra, rb) in runs_a.iter().zip(&runs_b) {
                assert_eq!(
                    (ra.text, &ra.format, ra.link),
                    (rb.text, &rb.format, rb.link),
                    "runs of {id}"
                );
            }
        }
        (ka, kb) => assert_eq!(ka, kb, "kind of {id}"),
    }
}

/// Every active revision the typed writers produced is reproduced by the page writer,
/// ignoring writer-allocated immutable styles and author records. Modification times are
/// not compared: a reorder can be attributed to either sibling, and the model cannot say which.
fn equivalent(reference: &[u8], written: &[u8]) {
    let store_a = Store::parse(reference).unwrap();
    let store_b = Store::parse(written).unwrap();
    let index_a = RevisionIndex::parse(&store_a).unwrap();
    let index_b = RevisionIndex::parse(&store_b).unwrap();
    index_b.validate_current().unwrap();
    let document_a = Document::parse(&index_a).unwrap();
    let document_b = Document::parse(&index_b).unwrap();
    assert_eq!(
        document_a.spaces.keys().collect::<Vec<_>>(),
        document_b.spaces.keys().collect::<Vec<_>>()
    );
    for sid in document_a.spaces.keys() {
        let a = document_a.active(*sid).unwrap();
        let b = document_b.active(*sid).unwrap();
        assert_eq!(a.roots, b.roots, "roots of {sid}");
        let roots: Vec<ExGuid> = a.roots.values().copied().collect();
        let reachable = |revision: &Revision<'_>| -> BTreeSet<ExGuid> {
            revision
                .parents(&roots)
                .unwrap()
                .into_keys()
                .chain(roots.iter().copied())
                .filter(|id| !matches!(revision.nodes[id].jcid, 0x12004d | 0x120001))
                .collect()
        };
        let ids_a = reachable(a);
        let ids_b = reachable(b);
        assert_eq!(ids_a, ids_b, "reachable objects of {sid}");
        for id in ids_a {
            same_element(a, b, id);
        }
    }
}

/// Runs the page writer on the model the typed writers produced and checks both contracts.
fn oracle(source: &[u8], space: ExGuid, reference: &[u8]) -> Vec<u8> {
    let after = page_in(reference, space);
    let prepared = PreparedEdit::page(source, space, &after, AUTHOR).unwrap();
    let written = prepared.as_bytes().to_vec();
    assert_eq!(page_in(&written, space), after, "model round trip");
    equivalent(reference, &written);
    let transactions = |bytes: &[u8]| Store::parse(bytes).unwrap().header.transaction_count;
    assert_eq!(
        transactions(&written),
        transactions(source) + 1,
        "one transaction"
    );
    written
}

#[test]
fn text_replacement_matches_replace_text() {
    let (space, page) = page_by_title(PARAGRAPHS, "Split middle");
    let (_, text, content) = text_ids(body(&page)[0])[0].clone();
    let end = u32::try_from(content.encode_utf16().count()).unwrap();
    for (range, replacement) in [
        (0..0, "🦀 "),
        (1..end.min(3), "é"),
        (end..end, " end"),
        (0..end, ""),
    ] {
        let reference =
            onestore::replace_text(PARAGRAPHS, space, text, range, replacement).unwrap();
        oracle(PARAGRAPHS, space, &reference);
    }
}

#[test]
fn formatting_matches_format_text() {
    let (space, page) = page_by_title(PARAGRAPHS, "Split style boundary");
    let (_, text, content) = text_ids(body(&page)[0])[0].clone();
    let end = u32::try_from(content.encode_utf16().count()).unwrap();
    let reference = PreparedEdit::format(
        PARAGRAPHS,
        space,
        text,
        1..end.min(4),
        &[
            TextAttribute::Bold(true),
            TextAttribute::Color(Some([255, 0, 0])),
            TextAttribute::FontSize(14.0),
        ],
    )
    .unwrap();
    let reference = PreparedEdit::format(
        reference.as_bytes(),
        space,
        text,
        0..1,
        &[
            TextAttribute::Italic(true),
            TextAttribute::Font("Consolas".into()),
        ],
    )
    .unwrap();
    oracle(PARAGRAPHS, space, reference.as_bytes());
}

#[test]
fn paragraph_insertions_match_insertion() {
    let (space, page) = page_by_title(OUTLINES, "Move subtree down");
    let outline = body(&page)[0];
    let paragraphs = text_ids(outline);
    let parent = outline
        .paragraphs
        .iter()
        .find(|p| p.parent.is_none())
        .unwrap()
        .id;
    for intent in [
        Insertion::paragraph(outline.id, None, "Appended 🦋 é", AUTHOR).unwrap(),
        Insertion::paragraph(outline.id, Some(paragraphs[0].0), "First", AUTHOR).unwrap(),
        Insertion::paragraph(parent, None, "Nested child", AUTHOR).unwrap(),
        Insertion::paragraph(outline.id, None, "", AUTHOR).unwrap(),
    ] {
        let reference = PreparedEdit::insert(OUTLINES, space, &intent).unwrap();
        oracle(OUTLINES, space, reference.as_bytes());
    }
}

#[test]
fn cell_insertion_and_deletion_match() {
    let (space, page) = page_by_title(TREES, "Delete cell subtree");
    let outline = body(&page)[0];
    let table = outline
        .paragraphs
        .iter()
        .find_map(|p| match &p.content {
            ParagraphContent::Table(table) => Some(table),
            _ => None,
        })
        .unwrap();
    let cell = &table.rows[0].cells[0];
    let intent = Insertion::paragraph(cell.id, None, "Cell text", AUTHOR).unwrap();
    let reference = PreparedEdit::insert(TREES, space, &intent).unwrap();
    oracle(TREES, space, reference.as_bytes());
    let victim = cell
        .paragraphs
        .iter()
        .find(|p| p.parent.is_none())
        .unwrap()
        .id;
    let reference =
        PreparedEdit::tree(TREES, space, &TreeEdit::delete(victim, AUTHOR).unwrap()).unwrap();
    oracle(TREES, space, reference.as_bytes());
}

#[test]
fn outline_insertion_matches_insertion() {
    let (space, _) = page_by_title(OUTLINES, "Move leaf down");
    let page = page_id(OUTLINES, space);
    let intent = Insertion::outline(page, 144.0, 200.0, "New outline 東京", AUTHOR).unwrap();
    let reference = PreparedEdit::insert(OUTLINES, space, &intent).unwrap();
    oracle(OUTLINES, space, reference.as_bytes());
}

#[test]
fn splits_and_joins_match_their_writers() {
    for title in ["Split middle", "Split start", "Split end", "Split parent"] {
        let (space, page) = page_by_title(PARAGRAPHS, title);
        let paragraphs = text_ids(body(&page)[0]);
        let (_, text, content) = paragraphs[0].clone();
        let units = u32::try_from(content.encode_utf16().count()).unwrap();
        let offset = match title {
            "Split start" => 0,
            "Split end" => units,
            _ => units / 2,
        };
        let offset = (0..=offset)
            .rev()
            .find(|at| {
                content
                    .encode_utf16()
                    .nth(at.wrapping_sub(1) as usize)
                    .is_none_or(|u| !(0xd800..=0xdbff).contains(&u))
            })
            .unwrap();
        let split = ParagraphSplit::new(text, offset, AUTHOR).unwrap();
        let reference = PreparedEdit::split(PARAGRAPHS, space, &split).unwrap();
        oracle(PARAGRAPHS, space, reference.as_bytes());
    }
    for title in ["Split middle", "Split empty"] {
        let (space, page) = page_by_title(PARAGRAPHS, title);
        let paragraphs = text_ids(body(&page)[0]);
        let join = ParagraphJoin::new(paragraphs[0].1, paragraphs[1].1, AUTHOR).unwrap();
        let reference = PreparedEdit::join(PARAGRAPHS, space, &join).unwrap();
        oracle(PARAGRAPHS, space, reference.as_bytes());
    }
}

#[test]
fn subtree_moves_and_deletions_match_tree_edits() {
    for (title, build) in [
        ("Move leaf down", 0usize),
        ("Move subtree down", 1),
        ("Delete leaf", 2),
        ("Delete subtree", 3),
        ("Move outline", 4),
        ("Delete outline", 5),
    ] {
        let (space, page) = page_by_title(OUTLINES, title);
        let outlines = body(&page);
        let outline = outlines[0];
        let top: Vec<ExGuid> = outline
            .paragraphs
            .iter()
            .filter(|p| p.parent.is_none())
            .map(|p| p.id)
            .collect();
        let edit = match build {
            0 | 1 => TreeEdit::move_to(top[0], outline.id, top.get(2).copied(), AUTHOR).unwrap(),
            2 | 3 => TreeEdit::delete(top[0], AUTHOR).unwrap(),
            4 => TreeEdit::move_to(outline.id, page_id(OUTLINES, space), None, AUTHOR).unwrap(),
            _ => TreeEdit::delete(outline.id, AUTHOR).unwrap(),
        };
        let reference = PreparedEdit::tree(OUTLINES, space, &edit).unwrap();
        oracle(OUTLINES, space, reference.as_bytes());
        if outlines.len() > 1 && build == 0 {
            let across = TreeEdit::move_to(top[1], outlines[1].id, None, AUTHOR).unwrap();
            let reference = PreparedEdit::tree(OUTLINES, space, &across).unwrap();
            oracle(OUTLINES, space, reference.as_bytes());
        }
    }
}

#[test]
fn outline_layout_and_collapse_match_outline_edits() {
    let (space, page) = page_by_title(OUTLINES, "Collapse subtree");
    let outline = body(&page)[0];
    let parent = outline
        .paragraphs
        .iter()
        .find(|p| !outline.paragraphs.iter().all(|q| q.parent != Some(p.id)))
        .unwrap()
        .id;
    for (object, edit) in [
        (outline.id, OutlineEdit::Position { x: 90.0, y: 250.5 }),
        (
            outline.id,
            OutlineEdit::Width {
                points: 300.0,
                user_set: true,
            },
        ),
        (
            outline.id,
            OutlineEdit::Width {
                points: 200.0,
                user_set: false,
            },
        ),
        (parent, OutlineEdit::Collapsed(true)),
    ] {
        let reference = PreparedEdit::outline(OUTLINES, space, object, edit).unwrap();
        oracle(OUTLINES, space, reference.as_bytes());
    }
}

#[test]
fn composed_edits_publish_one_transaction() {
    let (space, page) = page_by_title(OUTLINES, "Move subtree up");
    let outline = body(&page)[0];
    let paragraphs = text_ids(outline);
    let mut reference =
        onestore::replace_text(OUTLINES, space, paragraphs[0].1, 0..0, "Lead ").unwrap();
    let insertion =
        Insertion::paragraph(outline.id, Some(paragraphs[1].0), "Inserted", AUTHOR).unwrap();
    reference = PreparedEdit::insert(&reference, space, &insertion)
        .unwrap()
        .as_bytes()
        .to_vec();
    reference = PreparedEdit::format(
        &reference,
        space,
        insertion.text_object(),
        0..3,
        &[TextAttribute::Bold(true)],
    )
    .unwrap()
    .as_bytes()
    .to_vec();
    let last = paragraphs.last().unwrap().0;
    reference = PreparedEdit::tree(&reference, space, &TreeEdit::delete(last, AUTHOR).unwrap())
        .unwrap()
        .as_bytes()
        .to_vec();
    reference = PreparedEdit::outline(
        &reference,
        space,
        outline.id,
        OutlineEdit::Position { x: 60.0, y: 60.0 },
    )
    .unwrap()
    .as_bytes()
    .to_vec();
    let written = oracle(OUTLINES, space, &reference);
    assert_eq!(
        Store::parse(&reference).unwrap().header.transaction_count,
        Store::parse(OUTLINES).unwrap().header.transaction_count + 5
    );
    assert!(written.len() < reference.len());
}

#[test]
fn model_edits_round_trip_and_leave_other_objects_untouched() {
    let (space, mut after) = page_by_title(TREES, "Move numbered subtree down");
    let before = page_in(TREES, space);
    let page = page_id(TREES, space);
    let mut new_paragraph = None;
    for object in &mut after.objects {
        let PageObject::Outline(outline) = object else {
            continue;
        };
        if outline.title || new_paragraph.is_some() {
            continue;
        }
        let first = outline
            .paragraphs
            .iter()
            .position(|p| p.text().is_some())
            .unwrap();
        let text = outline.paragraphs[first].text_mut().unwrap();
        let end = text.text.utf16_offset(text.text.text().len()).unwrap();
        text.text
            .apply(onestore::page::text::Edit {
                range: end..end,
                replacement: onestore::page::Paragraph::new(
                    " appended 🦀".into(),
                    text.text.format_at(end).unwrap().clone(),
                ),
            })
            .unwrap();
        let mut fresh = outline.paragraphs[first].clone();
        fresh.id = new_id().unwrap();
        fresh.parent = None;
        fresh.level = 1;
        fresh.lists.clear();
        fresh.tags.clear();
        fresh.style = None;
        fresh.collapsed = false;
        let fresh_text = fresh.text_mut().unwrap();
        fresh_text.id = new_id().unwrap();
        fresh_text.tags.clear();
        fresh_text.text = onestore::page::Paragraph::new(
            "Brand new".into(),
            fresh_text.text.format_at(0).unwrap().clone(),
        );
        new_paragraph = Some(fresh.id);
        outline.paragraphs.push(fresh);
        outline.layout.x = Some(outline.layout.x.unwrap_or(0.0) + 18.0);
    }
    let mut outline = onestore::page::Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(300.0),
            y: Some(400.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: Vec::new(),
        unsupported: Vec::new(),
    };
    let template = body(&before)[0]
        .paragraphs
        .iter()
        .find(|p| p.text().is_some())
        .unwrap()
        .clone();
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    let text = paragraph.text_mut().unwrap();
    text.id = new_id().unwrap();
    text.tags.clear();
    text.text = onestore::page::Paragraph::new(
        "Outline text".into(),
        text.text.format_at(0).unwrap().clone(),
    );
    outline.paragraphs.push(paragraph);
    after.objects.push(PageObject::Outline(outline));
    let written = PreparedEdit::page(TREES, space, &after, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let normalize = |page: &Page| -> Vec<(ExGuid, Vec<(ExGuid, String)>)> {
        body(page)
            .iter()
            .map(|o| {
                (
                    o.id,
                    o.paragraphs
                        .iter()
                        .filter_map(|p| p.text().map(|t| (p.id, t.text.text().to_owned())))
                        .collect(),
                )
            })
            .collect()
    };
    assert_eq!(normalize(&stored), normalize(&after));
    assert_eq!(
        body(&stored)
            .iter()
            .map(|o| (o.layout.x, o.layout.y))
            .collect::<Vec<_>>(),
        body(&after)
            .iter()
            .map(|o| (o.layout.x, o.layout.y))
            .collect::<Vec<_>>()
    );
    let store = Store::parse(TREES).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let raw_before = index.resolve_active(space).unwrap();
    let store = Store::parse(written.as_bytes()).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let raw_after = index.resolve_active(space).unwrap();
    let touched: BTreeSet<ExGuid> = body(&after)
        .iter()
        .flat_map(|o| {
            std::iter::once(o.id).chain(
                o.paragraphs
                    .iter()
                    .flat_map(|p| [p.id, p.text().map(|t| t.id).unwrap_or(p.id)]),
            )
        })
        .chain([page])
        .collect();
    let mut untouched = 0;
    for (id, object) in &raw_before.objects {
        if touched.contains(id) || matches!(object.jcid, 0x12004d | 0x120001) {
            continue;
        }
        let after = &raw_after.objects[id];
        assert_eq!((object.jcid, object.data), (after.jcid, after.data), "{id}");
        untouched += 1;
    }
    assert!(untouched > 5, "{untouched}");
}

#[test]
fn unsupported_model_edits_are_rejected_before_writing() {
    let (space, page) = page_by_title(TREES, "Delete cell subtree");
    let mut widened = page_in(TREES, space);
    for object in &mut widened.objects {
        if let PageObject::Outline(outline) = object {
            for paragraph in &mut outline.paragraphs {
                if let ParagraphContent::Table(table) = &mut paragraph.content {
                    table.columns[0].width += 10.0;
                }
            }
        }
    }
    assert!(PreparedEdit::page(TREES, space, &widened, AUTHOR).is_ok());
    let mut relisted = page_in(TREES, space);
    let outline = body(&page)[0];
    for object in &mut relisted.objects {
        if let PageObject::Outline(o) = object
            && o.id == outline.id
        {
            o.paragraphs[0].lists.push(new_id().unwrap());
        }
    }
    assert!(PreparedEdit::page(TREES, space, &relisted, AUTHOR).is_err());
    let mut renamed = page_in(TREES, space);
    renamed.created = Some(1);
    assert!(PreparedEdit::page(TREES, space, &renamed, AUTHOR).is_err());
    assert!(PreparedEdit::page(TREES, space, &page, "a\0b").is_err());
}

/// Unset run values stay unset; only the language, which MS-ONE requires on every run, comes
/// from the insertion.
#[test]
fn a_new_paragraph_with_unspecified_formatting_rereads_with_only_the_required_language() {
    let (space, mut page) = page_by_title(OUTLINES, "Move leaf down");
    let text = onestore::page::TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new("Plain 東京".into(), Default::default()),
        tags: Vec::new(),
    };
    let mut paragraph = body(&page)[0].paragraphs[0].clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    paragraph.content = onestore::page::ParagraphContent::Text(text.clone());
    let outline = onestore::page::Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: onestore::document::Layout {
            x: Some(300.0),
            y: Some(500.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![paragraph],
        unsupported: Vec::new(),
    };
    page.objects.insert(0, PageObject::Outline(outline));
    let written = PreparedEdit::page(OUTLINES, space, &page, AUTHOR).unwrap();
    let stored = page_in(written.as_bytes(), space);
    let read_back = body(&stored)
        .iter()
        .flat_map(|outline| outline.paragraphs.iter())
        .find_map(|p| p.text().filter(|t| t.id == text.id))
        .unwrap();
    assert_eq!(read_back.text.text(), "Plain 東京");
    let format = read_back.text.format_at(0).unwrap();
    assert!(format.language.is_some());
    assert_eq!(
        format,
        &onestore::document::Format {
            language: format.language,
            ..Default::default()
        }
    );
    assert_eq!(
        PreparedEdit::page(written.as_bytes(), space, &stored, AUTHOR)
            .unwrap()
            .as_bytes(),
        written.as_bytes()
    );
}

#[test]
fn an_unchanged_model_publishes_nothing() {
    let (space, page) = page_by_title(OUTLINES, "Resize outline");
    let prepared = PreparedEdit::page(OUTLINES, space, &page, AUTHOR).unwrap();
    assert_eq!(prepared.as_bytes(), OUTLINES);
}

const JOIN_PAGE: &str = "Bold 🦀 italic e\u{301} color 東京";

fn native_candidate_edits() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "Move leaf down",
            "Anchor moved last and replaced with Unicode text, bold range on Trailing sibling",
        ),
        (
            "Move subtree down",
            "Target subtree moved last with a new nested child, italic blue 14 pt range on Anchor",
        ),
        (
            "Delete leaf",
            "Anchor deleted, paragraph appended, new outline with two paragraphs at (300, 400)",
        ),
        ("Delete subtree", "Anchor split in the middle"),
        (
            "Collapse subtree",
            "Target collapsed, outline moved to (90, 250.5) with a fixed 300 pt width",
        ),
        (
            "Move outline",
            "first body outline moved after the second, first paragraph of each extended",
        ),
        (
            "Delete outline",
            "second body outline deleted, underline, highlight and Consolas on a range of Anchor",
        ),
        (
            JOIN_PAGE,
            "the two paragraphs joined, strike and subscript on the first three units",
        ),
    ]
}

fn fresh(template: &onestore::page::PageParagraph, text: &str) -> onestore::page::PageParagraph {
    let mut paragraph = template.clone();
    paragraph.id = new_id().unwrap();
    paragraph.parent = None;
    paragraph.level = 1;
    paragraph.lists.clear();
    paragraph.tags.clear();
    paragraph.style = None;
    paragraph.collapsed = false;
    let format = template.text().unwrap().text.format_at(0).unwrap().clone();
    paragraph.content = ParagraphContent::Text(onestore::page::TextObject {
        id: new_id().unwrap(),
        date_field: None,
        text: onestore::page::Paragraph::new(text.into(), format),
        tags: Vec::new(),
    });
    paragraph
}

fn restyle(
    text: &mut onestore::page::TextObject,
    range: std::ops::Range<u32>,
    change: impl Fn(&mut onestore::document::Format),
) {
    let slice = text.text.slice(range.clone()).unwrap();
    let mut runs = Vec::new();
    let mut from = 0;
    for span in slice.spans() {
        let mut format = span.format.clone();
        change(&mut format);
        runs.push((slice.text()[from..span.end].to_owned(), format));
        from = span.end;
    }
    text.text
        .apply(onestore::page::text::Edit {
            range,
            replacement: onestore::page::Paragraph::from_runs(runs),
        })
        .unwrap();
}

fn set_text(text: &mut onestore::page::TextObject, replacement: &str) {
    let end = text.text.utf16_offset(text.text.text().len()).unwrap();
    let format = text.text.format_at(0).unwrap().clone();
    text.text
        .apply(onestore::page::text::Edit {
            range: 0..end,
            replacement: onestore::page::Paragraph::new(replacement.into(), format),
        })
        .unwrap();
}

/// Removes a top-level subtree and returns it in order.
fn take_subtree(
    outline: &mut onestore::page::Outline,
    index: usize,
) -> Vec<onestore::page::PageParagraph> {
    let mut ids = BTreeSet::from([outline.paragraphs[index].id]);
    loop {
        let before = ids.len();
        let more: Vec<ExGuid> = outline
            .paragraphs
            .iter()
            .filter(|p| p.parent.is_some_and(|q| ids.contains(&q)))
            .map(|p| p.id)
            .collect();
        ids.extend(more);
        if ids.len() == before {
            break;
        }
    }
    let (subtree, rest): (Vec<_>, Vec<_>) = outline
        .paragraphs
        .drain(..)
        .partition(|p| ids.contains(&p.id));
    outline.paragraphs = rest;
    subtree
}

fn expectation(page: &Page) -> serde_json::Value {
    let outlines: Vec<serde_json::Value> = body(page)
        .iter()
        .map(|outline| {
            let index: std::collections::BTreeMap<ExGuid, usize> =
                outline.paragraphs.iter().enumerate().map(|(i, p)| (p.id, i)).collect();
            serde_json::json!({
                "x": outline.layout.x,
                "y": outline.layout.y,
                "max_width": outline.layout.max_width,
                "width_set_by_user": outline.layout.width_set_by_user,
                "paragraphs": outline.paragraphs.iter().map(|p| serde_json::json!({
                    "text": p.text().map(|t| t.text.text()),
                    "parent": p.parent.map(|q| index[&q]),
                    "collapsed": p.collapsed,
                    "spans": p.text().map(|t| t.text.spans().iter().map(|s| serde_json::json!({
                        "end": t.text.utf16_offset(s.end).unwrap(),
                        "bold": s.format.bold, "italic": s.format.italic, "underline": s.format.underline,
                        "strike": s.format.strike, "subscript": s.format.subscript,
                        "font": s.format.font, "font_size": s.format.font_size,
                        "color": s.format.color, "highlight": s.format.highlight,
                    })).collect::<Vec<_>>()),
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::json!({ "title": page.title, "outlines": outlines })
}

#[test]
#[ignore = "exports a page-model candidate notebook for cold native validation"]
fn export_native_page_model_candidates() {
    let output = std::path::PathBuf::from(std::env::var_os("ONESTORE_PAGE_MODEL_OUTPUT").unwrap());
    assert!(output.is_absolute());
    std::fs::create_dir_all(output.join("candidate/notebook")).unwrap();
    let mut bytes = OUTLINES.to_vec();
    let mut cases = Vec::new();
    for (title, description) in native_candidate_edits() {
        let (space, _) = page_by_title(&bytes, title);
        let mut after = page_in(&bytes, space);
        let outlines: Vec<usize> = after
            .objects
            .iter()
            .enumerate()
            .filter(|(_, o)| matches!(o, PageObject::Outline(_)))
            .map(|(i, _)| i)
            .collect();
        fn body_outline(page: &mut Page, i: usize) -> &mut onestore::page::Outline {
            match &mut page.objects[i] {
                PageObject::Outline(outline) => outline,
                _ => unreachable!(),
            }
        }
        match title {
            "Move leaf down" => {
                let outline = body_outline(&mut after, outlines[0]);
                let subtree = take_subtree(outline, 0);
                outline.paragraphs.extend(subtree);
                let last = outline.paragraphs.len() - 1;
                set_text(
                    outline.paragraphs[last].text_mut().unwrap(),
                    "Rust 🦀 é 東京 שלום",
                );
                let trailing = outline
                    .paragraphs
                    .iter()
                    .position(|p| {
                        p.text()
                            .is_some_and(|t| t.text.text() == "Trailing sibling")
                    })
                    .unwrap();
                restyle(
                    outline.paragraphs[trailing].text_mut().unwrap(),
                    0..4,
                    |f| f.bold = Some(true),
                );
            }
            "Move subtree down" => {
                let outline = body_outline(&mut after, outlines[0]);
                let parent_index = outline
                    .paragraphs
                    .iter()
                    .position(|p| outline.paragraphs.iter().any(|q| q.parent == Some(p.id)))
                    .unwrap();
                let parent_id = outline.paragraphs[parent_index].id;
                let subtree = take_subtree(outline, parent_index);
                outline.paragraphs.extend(subtree);
                let mut child = fresh(&outline.paragraphs[0], "Nested by Rust");
                child.parent = Some(parent_id);
                child.level = 2;
                outline.paragraphs.push(child);
                restyle(outline.paragraphs[0].text_mut().unwrap(), 0..6, |f| {
                    f.italic = Some(true);
                    f.color = Some(0xff0000);
                    f.font_size = Some(14.0);
                });
            }
            "Delete leaf" => {
                let template = body(&after)[0].paragraphs[0].clone();
                let outline = body_outline(&mut after, outlines[0]);
                take_subtree(outline, 0);
                outline
                    .paragraphs
                    .push(fresh(&template, "Appended by Rust"));
                let new_outline = onestore::page::Outline {
                    id: new_id().unwrap(),
                    title: false,
                    min_width: None,
                    layout: onestore::document::Layout {
                        x: Some(300.0),
                        y: Some(400.0),
                        ..Default::default()
                    },
                    indents: Vec::new(),
                    paragraphs: vec![
                        fresh(&template, "Rust outline one"),
                        fresh(&template, "Rust outline two"),
                    ],
                    unsupported: Vec::new(),
                };
                let at = after
                    .objects
                    .iter()
                    .position(|o| matches!(o, PageObject::Title(_)))
                    .unwrap_or(after.objects.len());
                after.objects.insert(at, PageObject::Outline(new_outline));
            }
            "Delete subtree" => {
                let outline = body_outline(&mut after, outlines[0]);
                let first = outline.paragraphs[0].clone();
                let text = first.text().unwrap();
                let end = text.text.utf16_offset(text.text.text().len()).unwrap();
                let offset = (end / 2..end)
                    .find(|at| text.text.byte_offset(*at).is_ok())
                    .unwrap();
                let tail = text.text.slice(offset..end).unwrap();
                let mut right = fresh(&first, "");
                right.style = first.style;
                right.text_mut().unwrap().text = tail;
                let left = outline.paragraphs[0].text_mut().unwrap();
                left.text = left.text.slice(0..offset).unwrap();
                outline.paragraphs.insert(1, right);
            }
            "Collapse subtree" => {
                let outline = body_outline(&mut after, outlines[0]);
                let parent = outline
                    .paragraphs
                    .iter()
                    .position(|p| outline.paragraphs.iter().any(|q| q.parent == Some(p.id)))
                    .unwrap();
                outline.paragraphs[parent].collapsed = true;
                outline.layout.x = Some(90.0);
                outline.layout.y = Some(250.5);
                outline.layout.max_width = Some(300.0);
                outline.layout.width_set_by_user = Some(true);
            }
            "Move outline" => {
                assert!(outlines.len() >= 2);
                let first = after.objects.remove(outlines[0]);
                after.objects.insert(outlines[1], first);
                for i in [outlines[0], outlines[1]] {
                    let outline = body_outline(&mut after, i);
                    let text = outline.paragraphs[0].text_mut().unwrap();
                    let end = text.text.utf16_offset(text.text.text().len()).unwrap();
                    let format = text.text.format_at(end).unwrap().clone();
                    text.text
                        .apply(onestore::page::text::Edit {
                            range: end..end,
                            replacement: onestore::page::Paragraph::new(" (Rust)".into(), format),
                        })
                        .unwrap();
                }
            }
            "Delete outline" => {
                assert!(outlines.len() >= 2);
                after.objects.remove(outlines[1]);
                let outline = body_outline(&mut after, outlines[0]);
                restyle(outline.paragraphs[0].text_mut().unwrap(), 1..5, |f| {
                    f.underline = Some(true);
                    f.highlight = Some(0x00ffff);
                    f.font = Some("Consolas".into());
                });
            }
            _ => {
                let outline = body_outline(&mut after, outlines[0]);
                assert_eq!(outline.paragraphs.len(), 2);
                let right = outline.paragraphs.remove(1);
                outline.paragraphs[0]
                    .text_mut()
                    .unwrap()
                    .text
                    .append(right.text().unwrap().text.clone())
                    .unwrap();
                restyle(outline.paragraphs[0].text_mut().unwrap(), 0..3, |f| {
                    f.strike = Some(true);
                    f.subscript = Some(true);
                });
            }
        }
        let edit = PreparedEdit::page(&bytes, space, &after, "Rust page writer").unwrap();
        bytes = edit.as_bytes().to_vec();
        let stored = page_in(&bytes, space);
        let index = {
            let store = Store::parse(&bytes).unwrap();
            let index = RevisionIndex::parse(&store).unwrap();
            let document = Document::parse(&index).unwrap();
            document
                .pages()
                .unwrap()
                .iter()
                .position(|(sid, _)| *sid == space)
                .unwrap()
        };
        cases.push(serde_json::json!({
            "title": title,
            "index": index,
            "description": description,
            "space": space.to_string(),
            "expected": expectation(&stored),
        }));
    }
    std::fs::write(output.join("candidate/notebook/synthetic.one"), &bytes).unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../corpus/outline-edit/before/notebook/Open Notebook.onetoc2"
        ),
        output.join("candidate/notebook/Open Notebook.onetoc2"),
    )
    .unwrap();
    std::fs::write(
        output.join("candidate/manifest.json"),
        serde_json::to_string_pretty(&serde_json::json!({
            "source": "corpus/outline-edit/before/notebook/synthetic.one",
            "author": "Rust page writer",
            "transactions_added": cases.len(),
            "cases": cases,
        }))
        .unwrap(),
    )
    .unwrap();
}
