//! Formatting and insertion as page-model differences: what reconciles with an independent
//! remote change, what keeps a conflict page, and what the edit made again publishes.

#[path = "../../onestore/tests/support/ops.rs"]
mod ops;

use notebook::{EditStatus, Replica};
use onestore::{
    ExGuid, RevisionIndex, Store,
    document::{Document, Format},
    page::{Page, PageObject},
};

#[path = "support/server.rs"]
mod server;
use server::{Fault, Server, conflicted, pages, snapshot};
#[path = "support/model_ops.rs"]
mod model_ops;
#[path = "../../onestore/tests/support/sweep.rs"]
mod sweep;
use model_ops::{
    AUTHOR, insert_after, insert_outline, page_of, paragraph_with, replace_text, restyle, save,
};

const OUTLINES: &[u8] =
    include_bytes!("../../../corpus/outline-edit/before/notebook/synthetic.one");
const PAGE: &str = "Move subtree down";

fn page_titled(bytes: &[u8], title: &str) -> (ExGuid, Page) {
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

/// The text objects of a page that carry no hidden field, and so accept range formatting.
fn plain(page: &Page) -> Vec<ExGuid> {
    page.objects
        .iter()
        .flat_map(|object| match object {
            PageObject::Outline(outline) => outline.paragraphs.iter().collect(),
            _ => Vec::new(),
        })
        .filter_map(|paragraph| {
            let text = paragraph.text()?;
            (!text.text.text().contains('\u{fddf}')).then_some(text.id)
        })
        .collect()
}

fn text_of(page: &Page, text: ExGuid) -> String {
    paragraph_with(page, text)
        .unwrap()
        .text()
        .unwrap()
        .text
        .text()
        .to_owned()
}

/// The format of every UTF-16 unit of a text object.
fn formats(page: &Page, text: ExGuid) -> Vec<Format> {
    let content = &paragraph_with(page, text).unwrap().text().unwrap().text;
    let mut out = Vec::new();
    let mut from = 0;
    for span in content.spans() {
        let units = content.text()[from..span.end].encode_utf16().count();
        out.extend(std::iter::repeat_n(span.format.clone(), units));
        from = span.end;
    }
    out
}

/// A remote holding the fixture page with `change` applied by a native author.
fn remote_with(space: ExGuid, change: impl FnOnce(&mut Page)) -> Server {
    let mut page = page_of(OUTLINES, space);
    change(&mut page);
    Server::new(
        ops::saved(OUTLINES, space, &page)
            .unwrap()
            .as_slice(),
    )
}

fn cache(directory: &tempfile::TempDir) -> Replica {
    Replica::create(directory.path().join("cache.sqlite"), OUTLINES).unwrap()
}

#[test]
fn offline_formatting_merges_with_a_remote_edit_to_another_paragraph() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let id = save(&cache, ids[0], |page| {
        restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
    })
    .unwrap()
    .unwrap();
    let mut server = remote_with(space, |page| replace_text(page, ids[3], 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
    );
    let published = page_of(&server.durable, space);
    assert!(
        formats(&published, ids[0])
            .iter()
            .all(|format| format.bold == Some(true))
    );
    assert_eq!(text_of(&published, ids[3]), "Remote Trailing sibling");
    assert_eq!(snapshot(&cache), server.durable);
}

#[test]
fn offline_formatting_conflicts_only_with_remote_changes_it_overlaps() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    for typed in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        let id = save(&cache, ids[0], |page| {
            restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
        })
        .unwrap()
        .unwrap();
        let local = snapshot(&cache);
        let mut server = remote_with(space, |page| {
            if typed {
                replace_text(page, ids[0], 0..0, "Remote ");
            } else {
                restyle(page, ids[0], 0..3, |format| format.italic = Some(true));
            }
        });
        let outcome = cache.sync_once(&mut server).unwrap().edit;
        if typed {
            // Text typed before the formatted range moves it; the ranges do not meet.
            assert!(matches!(outcome, Some((n, EditStatus::Published { .. })) if n == id));
            let published = page_of(&server.durable, space);
            assert!(text_of(&published, ids[0]).starts_with("Remote Anchor"));
            let styles = formats(&published, ids[0]);
            for (offset, format) in styles.iter().enumerate() {
                assert_eq!(
                    format.bold == Some(true),
                    (7..13).contains(&offset),
                    "{offset}"
                );
            }
        } else {
            assert!(matches!(outcome, Some((_, EditStatus::Published { .. }))));
            assert!(conflicted(&server.durable, space));
            let remote = page_of(&server.durable, space);
            assert!(formats(&remote, ids[0]).iter().all(|format| format.bold != Some(true)));
            assert_eq!(pages(&snapshot(&cache)), pages(&server.durable));
            assert_ne!(pages(&local), pages(&server.durable));
        }
    }
}

#[test]
fn competing_formatting_of_one_paragraph_keeps_a_conflict_page_until_made_again() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("competing.sqlite");
    let cache = Replica::create(&path, OUTLINES).unwrap();
    let id = save(&cache, ids[0], |page| {
        restyle(page, ids[0], 0..6, |format| format.font_size = Some(14.0));
    })
    .unwrap()
    .unwrap();
    let mut server = remote_with(space, |page| {
        restyle(page, ids[0], 0..3, |format| format.italic = Some(true));
    });
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    assert!(conflicted(&server.durable, space));
    assert_eq!(cache.sync_once(&mut server).unwrap().edit, None);
    drop(cache);
    let cache = Replica::open(&path).unwrap();
    assert!(matches!(
        cache.status(id).unwrap(),
        Some(EditStatus::Published { .. })
    ));
    save(&cache, ids[0], |page| {
        restyle(page, ids[0], 0..6, |format| format.font_size = Some(14.0));
    })
    .unwrap();
    assert!(matches!(
        cache.sync_once(&mut server).unwrap().edit,
        Some((_, EditStatus::Published { .. }))
    ));
    let published = formats(&page_of(&server.durable, space), ids[0]);
    assert!(
        published
            .iter()
            .all(|format| format.font_size == Some(14.0))
    );
    assert_eq!(
        published
            .iter()
            .filter(|format| format.italic == Some(true))
            .count(),
        3
    );
}

#[test]
fn every_visual_attribute_publishes_through_a_page_model_save() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    for (name, apply) in [
        (
            "bold",
            (|f: &mut Format| f.bold = Some(true)) as fn(&mut Format),
        ),
        ("italic", |f| f.italic = Some(true)),
        ("underline", |f| f.underline = Some(true)),
        ("strike", |f| f.strike = Some(true)),
        ("superscript", |f| f.superscript = Some(true)),
        ("subscript", |f| f.subscript = Some(true)),
        ("font", |f| f.font = Some("Georgia".into())),
        ("font size", |f| f.font_size = Some(21.0)),
        ("color", |f| f.color = Some(0x0056_3412)),
        ("highlight", |f| f.highlight = Some(0x0006_0504)),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        let id = save(&cache, ids[0], |page| restyle(page, ids[0], 2..6, apply))
            .unwrap()
            .unwrap();
        let mut server = Server::new(OUTLINES);
        assert!(
            matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id),
            "{name}"
        );
        let mut expected = formats(&page_of(OUTLINES, space), ids[0]);
        assert_eq!(expected.len(), 6);
        for format in &mut expected[2..] {
            apply(format);
        }
        assert_eq!(
            formats(&page_of(&server.durable, space), ids[0]),
            expected,
            "{name}"
        );
    }
}

#[test]
fn seeded_formatting_reconciles_exactly_when_the_remote_left_the_paragraph_alone() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let lengths: Vec<u32> = ids
        .iter()
        .map(|id| u32::try_from(text_of(&page, *id).encode_utf16().count()).unwrap())
        .collect();
    let (mut published, mut conflicts) = (0, 0);
    let seeds = sweep::seeds(1..65, 16);
    for seed in seeds.clone() {
        let mut state = seed;
        let mut next = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            usize::try_from(state >> 33).unwrap()
        };
        let target = next() % ids.len();
        let start = u32::try_from(next()).unwrap() % lengths[target];
        let end = start + 1 + u32::try_from(next()).unwrap() % (lengths[target] - start);
        let attribute: fn(&mut Format) = match seed % 3 {
            0 => |format| format.bold = Some(true),
            1 => |format| format.font_size = Some(21.0),
            _ => |format| format.color = Some(0x0012_3456),
        };
        let remote: Vec<usize> = (0..1 + next() % 3).map(|_| next() % ids.len()).collect();
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        let id = save(&cache, ids[target], |page| {
            restyle(page, ids[target], start..end, attribute);
        })
        .unwrap()
        .unwrap();
        let local = snapshot(&cache);
        let mut server = remote_with(space, |page| {
            for (step, &at) in remote.iter().enumerate() {
                if step % 2 == 0 {
                    replace_text(page, ids[at], 0..0, "\u{2022}");
                } else {
                    restyle(page, ids[at], 0..1, |format| format.underline = Some(true));
                }
            }
        });
        let remote_page = page_of(&server.durable, space);
        let outcome = cache.sync_once(&mut server).unwrap().edit.unwrap();
        assert_eq!(outcome.0, id, "seed {seed}");
        // Bullets the remote typed before the target's text, and whether it underlined a
        // character the target already had.
        let mut bullets = vec![0_u32; ids.len()];
        let mut underlined = vec![false; ids.len()];
        for (step, &at) in remote.iter().enumerate() {
            if step % 2 == 0 {
                bullets[at] += 1;
            } else if bullets[at] == 0 {
                underlined[at] = true;
            }
        }
        if underlined[target] && start == 0 {
            conflicts += 1;
            assert!(matches!(outcome.1, EditStatus::Published { .. }), "seed {seed}");
            assert!(conflicted(&server.durable, space), "seed {seed}");
            assert_eq!(page_of(&server.durable, space), remote_page, "seed {seed}");
            assert_ne!(pages(&snapshot(&cache)), pages(&local), "seed {seed}");
            continue;
        }
        published += 1;
        assert!(
            matches!(outcome.1, EditStatus::Published { .. }),
            "seed {seed}: {outcome:?}"
        );
        let page = page_of(&server.durable, space);
        let shift = bullets[target] as usize;
        let mut expected: Vec<Format> = formats(&remote_page, ids[target]);
        for format in &mut expected[start as usize + shift..end as usize + shift] {
            attribute(format);
        }
        assert_eq!(formats(&page, ids[target]), expected, "seed {seed}");
        for &at in &remote {
            assert!(
                text_of(&page, ids[at]).starts_with('\u{2022}')
                    || formats(&page, ids[at])[0].underline == Some(true),
                "seed {seed}"
            );
        }
        assert_eq!(snapshot(&cache), server.durable, "seed {seed}");
    }
    assert!(
        published * 4 >= seeds.end - seeds.start,
        "{published} published, {conflicts} conflicts"
    );
}

#[test]
fn offline_insertions_survive_reopen_and_carry_their_formatting() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("insert.sqlite");
    let mut cache = Replica::create(&path, OUTLINES).unwrap();
    let id = save(&cache, ids[0], |page| {
        insert_outline(page, 72.0, 144.0, "Offline outline");
        insert_after(page, ids[0], "Offline paragraph 🦀");
    })
    .unwrap()
    .unwrap();
    let local = snapshot(&cache);
    let pending = cache.pending().unwrap();
    drop(cache);
    cache = Replica::open(&path).unwrap();
    assert_eq!(pages(&snapshot(&cache)), pages(&local));
    assert_eq!(cache.pending().unwrap(), pending);
    let mut server = remote_with(space, |page| replace_text(page, ids[3], 0..0, "Remote "));
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
    );
    let published = page_of(&server.durable, space);
    let inserted = plain(&published)
        .into_iter()
        .find(|id| text_of(&published, *id) == "Offline paragraph 🦀")
        .unwrap();
    assert!(
        plain(&published)
            .iter()
            .any(|id| text_of(&published, *id) == "Offline outline")
    );
    assert_eq!(text_of(&published, ids[3]), "Remote Trailing sibling");
    let styled = save(&cache, inserted, |page| {
        restyle(page, inserted, 8..17, |format| format.bold = Some(true));
    })
    .unwrap()
    .unwrap();
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == styled)
    );
    let styles = formats(&page_of(&server.durable, space), inserted);
    for (offset, format) in styles.iter().enumerate() {
        assert_eq!(format.bold == Some(true), (8..17).contains(&offset));
    }
    assert_eq!(snapshot(&cache), server.durable);
}

#[test]
fn uncertain_formatting_attempts_keep_the_original_attempt_and_never_replay() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    for fault in [Fault::UnknownBefore, Fault::UnknownAfter] {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("uncertain.sqlite");
        let cache = Replica::create(&path, OUTLINES).unwrap();
        let id = save(&cache, ids[0], |page| {
            restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
        })
        .unwrap()
        .unwrap();
        let local = snapshot(&cache);
        let mut server = Server::new(OUTLINES);
        server.fault = fault;
        assert!(cache.sync_once(&mut server).is_err());
        let state = cache.status(id).unwrap().unwrap();
        assert!(matches!(state, EditStatus::AwaitingConfirmation { .. }));
        drop(cache);
        let cache = Replica::open(&path).unwrap();
        assert_eq!(cache.status(id).unwrap(), Some(state.clone()));
        if matches!(fault, Fault::UnknownBefore) {
            for _ in 0..3 {
                assert_eq!(
                    cache.sync_once(&mut server).unwrap().edit,
                    Some((id, state.clone()))
                );
            }
            assert_eq!(server.publications, 1);
            assert_eq!(server.durable, OUTLINES);
            assert_eq!(pages(&snapshot(&cache)), pages(&local));
            continue;
        }
        assert!(matches!(
            cache.sync_once(&mut server).unwrap().edit,
            Some((_, EditStatus::Published { .. }))
        ));
        assert_eq!(server.publications, 1);
        assert!(cache.pending().unwrap().is_empty());
        assert!(
            formats(&page_of(&server.durable, space), ids[0])
                .iter()
                .all(|format| format.bold == Some(true))
        );
    }
}

#[test]
fn a_format_the_remote_already_carries_is_confirmed_without_republishing() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let id = save(&cache, ids[0], |page| {
        restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
    })
    .unwrap()
    .unwrap();
    let mut server = remote_with(space, |page| {
        restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
    });
    server.durable = OUTLINES.to_vec();
    server.fault = Fault::Confirm;
    assert!(cache.sync_once(&mut server).is_err());
    assert_eq!(cache.status(id).unwrap(), Some(EditStatus::Pending));
    assert_eq!(server.durable, OUTLINES);
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
    );
    assert_eq!((server.publications, server.confirmations), (0, 2));
    assert!(cache.pending().unwrap().is_empty());
    let published = page_of(&snapshot(&cache), space);
    assert_eq!(published, page_of(&server.durable, space));
    assert!(
        formats(&published, ids[0])
            .iter()
            .all(|format| format.bold == Some(true))
    );
}

#[test]
fn a_retired_attempt_confirms_only_once_the_remote_page_equals_it() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let mut after = page.clone();
    restyle(&mut after, ids[0], 0..6, |format| format.bold = Some(true));
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    let id = model_ops::save_as(&cache, space, &after, AUTHOR)
        .unwrap()
        .unwrap();
    let mut server = Server::new(OUTLINES);
    server.fault = Fault::UnknownAfter;
    assert!(cache.sync_once(&mut server).is_err());
    let attempt = cache.status(id).unwrap();
    assert!(matches!(
        attempt,
        Some(EditStatus::AwaitingConfirmation { .. })
    ));
    let partial = remote_with(space, |page| {
        restyle(page, ids[0], 0..3, |format| format.bold = Some(true));
    });
    server.visible.clone_from(&partial.visible);
    assert_eq!(
        cache.sync_once(&mut server).unwrap().edit,
        attempt.map(|state| (id, state))
    );
    assert_eq!(server.confirmations, 0);
    let complete = remote_with(space, |page| {
        restyle(page, ids[0], 0..6, |format| format.bold = Some(true));
    });
    server.visible.clone_from(&complete.visible);
    assert_eq!(page_of(&server.visible, space), after);
    assert!(
        matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
    );
    assert_eq!((server.publications, server.confirmations), (1, 1));
    assert!(cache.pending().unwrap().is_empty());
}

#[test]
fn a_formatted_save_of_an_unchanged_model_queues_nothing() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let directory = tempfile::tempdir().unwrap();
    let cache = cache(&directory);
    assert_eq!(
        model_ops::save_as(&cache, space, &page, AUTHOR).unwrap(),
        None
    );
    assert!(cache.pending().unwrap().is_empty());
}

#[test]
fn paragraph_formatting_survives_reopen_rebase_and_a_lost_publication_reply() {
    let (space, page) = page_titled(OUTLINES, PAGE);
    let ids = plain(&page);
    let attributes: &[fn(&mut Format)] = &[
        |f| f.alignment = Some(2),
        |f| f.rtl = Some(true),
        |f| f.space_before = Some(12.0),
        |f| f.space_after = Some(6.0),
        |f| f.line_spacing = Some(18.0),
    ];
    for apply in attributes {
        let directory = tempfile::tempdir().unwrap();
        let cache = cache(&directory);
        let id = save(&cache, ids[0], |page| restyle(page, ids[0], 0..6, apply))
            .unwrap()
            .unwrap();
        let expected = formats(&page_of(&snapshot(&cache), space), ids[0]);
        drop(cache);
        let cache = Replica::open(directory.path().join("cache.sqlite")).unwrap();
        let mut server = remote_with(space, |page| replace_text(page, ids[3], 0..0, "Remote "));
        server.fault = Fault::UnknownAfter;
        assert!(
            matches!(cache.sync_once(&mut server), Err(notebook::Error::Remote(error)) if error.state == onestore::CommitState::Unknown)
        );
        assert!(matches!(
            cache.status(id).unwrap(),
            Some(EditStatus::AwaitingConfirmation { .. })
        ));
        drop(cache);
        let cache = Replica::open(directory.path().join("cache.sqlite")).unwrap();
        assert!(
            matches!(cache.sync_once(&mut server).unwrap().edit, Some((actual, EditStatus::Published { .. })) if actual == id)
        );
        assert_eq!(server.publications, 1);
        let page = page_of(&server.durable, space);
        assert_eq!(formats(&page, ids[0]), expected);
        assert!(text_of(&page, ids[3]).starts_with("Remote "));
    }
}
