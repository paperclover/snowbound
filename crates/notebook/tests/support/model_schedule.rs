//! A deterministic multi-actor schedule over page-model saves: several replicas edit one
//! page offline, publish through a fault-injecting remote, reopen, and delete the conflict
//! pages the merges leave.

use crate::model_ops::{self, AUTHOR};
use crate::server::{Fault, Server, remote_snapshot, snapshot};
use notebook::{EditStatus, Remote, Replica};
use onestore::{
    CommitError, ExGuid, RevisionIndex, Stamp, Store, Transaction,
    document::Document,
    page::{Outline, Page, PageObject},
};
use std::{io, sync::LazyLock};

#[path = "../../../onestore/tests/support/ops.rs"]
mod ops;

static SOURCE: LazyLock<(Vec<u8>, ExGuid)> = LazyLock::new(|| {
    let source =
        onestore::create_section("model-schedule.one", "Original 🦀 é 0", "Author").unwrap();
    let store = Store::parse(&source).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    let document = Document::parse(&index).unwrap();
    let space = document.pages().unwrap()[0].0;
    let mut page = Page::from_space(&document, space).unwrap();
    let mut anchor = body(&page).paragraphs[0].text().unwrap().id;
    for at in 1..8 {
        anchor = model_ops::insert_after(&mut page, anchor, &format!("Original 🦀 é {at}")).1;
    }
    let source = ops::saved(&source, space, &page)
        .unwrap()
        .as_slice()
        .to_vec();
    (source, space)
});

fn body(page: &Page) -> &Outline {
    page.objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .expect("the page keeps one body outline")
}

/// Everything the schedule asserts about a page, free of the identities the writers mint.
fn shape(page: &Page) -> Vec<String> {
    let outline = body(page);
    let mut out = vec![format!("{:?}", outline.layout)];
    out.extend(outline.paragraphs.iter().map(|paragraph| {
        format!(
            "{} {} {} {:?}",
            paragraph.level,
            u8::from(paragraph.collapsed),
            u8::from(paragraph.parent.is_some()),
            paragraph.text().map(|text| text.text.text()),
        )
    }));
    out
}

/// Paragraph and text identities of the body outline with their text, in model order.
fn rows(bytes: &[u8]) -> Vec<(ExGuid, ExGuid, String)> {
    let page = model_ops::page_of(bytes, SOURCE.1);
    body(&page)
        .paragraphs
        .iter()
        .filter_map(|paragraph| {
            let text = paragraph.text()?;
            Some((paragraph.id, text.id, text.text.text().to_owned()))
        })
        .collect()
}

/// Moves a paragraph and its descendants immediately before `anchor`, or last among
/// `parent`'s children; `anchor` supplies its own parent and level.
pub(crate) fn move_subtree(
    page: &mut Page,
    paragraph: ExGuid,
    parent: Option<ExGuid>,
    anchor: Option<ExGuid>,
) {
    for outline in model_ops::outlines_mut(page) {
        let Some(at) = outline.paragraphs.iter().position(|p| p.id == paragraph) else {
            continue;
        };
        let level = outline.paragraphs[at].level;
        let mut end = at + 1;
        while end < outline.paragraphs.len() && outline.paragraphs[end].level > level {
            end += 1;
        }
        if parent
            .into_iter()
            .chain(anchor)
            .any(|id| outline.paragraphs[at..end].iter().any(|p| p.id == id))
        {
            return;
        }
        let mut subtree: Vec<_> = outline.paragraphs.drain(at..end).collect();
        let (destination, parent, depth) = match anchor {
            Some(anchor) => {
                let Some(at) = outline.paragraphs.iter().position(|p| p.id == anchor) else {
                    return;
                };
                let target = &outline.paragraphs[at];
                (at, target.parent, target.level)
            }
            None => match parent {
                Some(parent) => {
                    let Some(at) = outline.paragraphs.iter().position(|p| p.id == parent) else {
                        return;
                    };
                    let level = outline.paragraphs[at].level;
                    let mut end = at + 1;
                    while end < outline.paragraphs.len() && outline.paragraphs[end].level > level {
                        end += 1;
                    }
                    (end, Some(parent), level + 1)
                }
                None => (outline.paragraphs.len(), None, 1),
            },
        };
        subtree[0].parent = parent;
        for descendant in &mut subtree {
            descendant.level = descendant.level + depth - level;
        }
        outline.paragraphs.splice(destination..destination, subtree);
        return;
    }
}

/// Asserts the publication invariants, then delegates.
struct Session<'a> {
    server: &'a mut Server,
    /// The head batch was already attempted, so its publication must never be repeated.
    retired: bool,
    publications: usize,
}

impl Remote for Session<'_> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.server.read()
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        self.server.stamp()
    }

    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        assert!(!self.retired, "a retired attempt was replayed");
        self.publications += 1;
        assert_eq!(self.publications, 1);
        self.server.publish(transaction)
    }

    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        self.server.confirm(base)
    }
}

/// The page's shape as the replica's queue leaves it.
fn local(cache: &Replica) -> Vec<String> {
    shape(&cache.page(SOURCE.1).unwrap())
}

fn statuses(cache: &Replica) -> Vec<(u64, Option<EditStatus>)> {
    cache
        .pending()
        .unwrap()
        .iter()
        .map(|edit| (edit.id, cache.status(edit.id).unwrap()))
        .collect()
}

/// Fails unless the durable remote holds `revision`: in the edited page's space, or, for
/// the edit adding a conflict page, in the space it changed first.
fn durable(server: &Server, revision: ExGuid) {
    let store = Store::parse(&server.durable).unwrap();
    let index = RevisionIndex::parse(&store).unwrap();
    assert!(
        index
            .spaces
            .values()
            .any(|space| space.revisions.contains_key(&revision))
    );
}

pub fn run(input: &[u8]) {
    let (source, space) = &*SOURCE;
    let directory = tempfile::tempdir().unwrap();
    let mut replicas: [Option<Replica>; 12] = std::array::from_fn(|_| None);
    let mut server = Server::new(source);
    for step in input.chunks_exact(8).take(48) {
        let actor = usize::from(step[0]) % replicas.len();
        let path = directory.path().join(format!("{actor}.sqlite"));
        let cache = replicas[actor].get_or_insert_with(|| Replica::create(&path, source).unwrap());
        let image = snapshot(cache);
        let before = local(cache);
        let pending = cache.pending().unwrap();
        let rows_now = rows(&image);
        match step[1] % 8 {
            0..=2 => {
                let row = rows_now[usize::from(step[2]) % rows_now.len()].clone();
                let other = rows_now[usize::from(step[4]) % rows_now.len()].clone();
                let change: Box<dyn Fn(&mut Page)> = match step[3] % 6 {
                    0 if !row.2.is_empty() => Box::new(move |page| {
                        model_ops::replace_text(
                            page,
                            row.1,
                            0..1,
                            &char::from(b'A' + step[5] % 26).to_string(),
                        );
                    }),
                    1 => Box::new(move |page| {
                        model_ops::insert_after(page, row.1, "Inserted 🦋 é");
                    }),
                    2 => Box::new(move |page| {
                        move_subtree(page, row.0, (step[5] & 1 != 0).then_some(other.0), None);
                    }),
                    3 => Box::new(move |page| model_ops::delete_paragraph(page, row.1)),
                    4 => Box::new(move |page| {
                        let outline = model_ops::outlines_mut(page).swap_remove(0);
                        if step[5] & 1 == 0 {
                            outline.layout.x = Some(f32::from(step[5]) + 36.0);
                            outline.layout.y = Some(f32::from(step[6]) + 36.0);
                        } else {
                            outline.layout.max_width = Some(f32::from(step[6]) + 72.0);
                            outline.layout.width_set_by_user = Some(step[6] & 2 == 0);
                        }
                    }),
                    5 if !row.2.is_empty() => Box::new(move |page| {
                        model_ops::restyle(page, row.1, 0..1, |format| {
                            format.font_size = Some(f32::from(step[5] % 20) + 12.0);
                        });
                    }),
                    _ => Box::new(|_| {}),
                };
                let mut after = cache.page(*space).unwrap();
                change(&mut after);
                match model_ops::save_as(cache, *space, &after, AUTHOR) {
                    Ok(id) => {
                        let next = cache.pending().unwrap();
                        let ids = |edits: &[notebook::PendingEdit]| -> Vec<u64> {
                            edits.iter().map(|edit| edit.id).collect()
                        };
                        assert_eq!(ids(&next)[..pending.len()], ids(&pending)[..]);
                        assert_eq!(next.len(), pending.len() + usize::from(id.is_some()));
                        assert_eq!(local(cache), shape(&after));
                    }
                    Err(_) => {
                        assert_eq!(local(cache), before);
                        assert_eq!(cache.pending().unwrap(), pending);
                    }
                }
            }
            3 | 4 => {
                server.fault = match step[2] % 8 {
                    0 => Fault::Before,
                    1 => Fault::UnknownBefore,
                    2 => Fault::UnknownAfter,
                    3 => Fault::Committed,
                    4 => Fault::Confirm,
                    5 => Fault::ConfirmCommitted,
                    _ => Fault::None,
                };
                let head = pending.first().map(|edit| edit.id);
                let result = {
                    let mut session = Session {
                        retired: head.is_some_and(|id| {
                            matches!(
                                cache.status(id).unwrap(),
                                Some(EditStatus::AwaitingConfirmation { .. })
                            )
                        }),
                        server: &mut server,
                        publications: 0,
                    };
                    cache.sync_once(&mut session)
                };
                server.fault = Fault::None;
                let next = cache.pending().unwrap();
                // Edits leave the queue oldest first, each with a durable receipt; a rebase
                // queues the conflict pages it makes after them.
                let queued = |edit: &notebook::PendingEdit| pending.iter().any(|kept| kept.id == edit.id);
                let left = pending.len() - next.iter().filter(|edit| queued(edit)).count();
                assert!(next.iter().all(|edit| {
                    pending[left..].iter().any(|kept| kept.id == edit.id)
                        || matches!(
                            &edit.edit.ops[..],
                            [onestore::op::Op::Section(onestore::op::SectionOp::Conflict { .. })]
                        )
                }));
                for edit in &pending[..left] {
                    let Some(EditStatus::Published { revision }) = cache.status(edit.id).unwrap()
                    else {
                        panic!("an edit leaves the queue only with its receipt")
                    };
                    durable(&server, revision);
                }
                if next.is_empty() && result.is_ok() {
                    assert_eq!(
                        local(cache),
                        shape(&model_ops::page_of(
                            &remote_snapshot(cache),
                            *space
                        ))
                    );
                }
            }
            5 => {
                let remote = rows(&server.visible);
                let row = remote[usize::from(step[2]) % remote.len()].clone();
                if !row.2.is_empty() {
                    let mut page = model_ops::page_of(&server.visible, *space);
                    model_ops::replace_text(
                        &mut page,
                        row.1,
                        0..1,
                        &char::from(b'a' + step[3] % 26).to_string(),
                    );
                    let visible = server.visible.clone();
                    ops::save(&visible, *space, &page)
                        .unwrap()
                        .commit(&mut server)
                        .unwrap();
                }
            }
            6 => {
                // The writer merged a conflict page's version by hand and deletes it.
                let listed = cache.conflicts().unwrap();
                if let Some(conflict) = listed
                    .iter()
                    .find(|(page, _)| page == space)
                    .map(|(_, pages)| pages[usize::from(step[2]) % pages.len()].space)
                {
                    let delete = onestore::op::Op::Section(onestore::op::SectionOp::Delete(vec![
                        conflict,
                    ]));
                    cache
                        .apply(
                            AUTHOR,
                            onestore::op::Edit {
                                at: model_ops::now(),
                                ops: vec![delete],
                            },
                        )
                        .unwrap();
                    assert!(
                        cache
                            .conflicts()
                            .unwrap()
                            .iter()
                            .flat_map(|(_, pages)| pages)
                            .all(|page| page.space != conflict)
                    );
                    assert_eq!(local(cache), before);
                    let (result, publications) = {
                        let mut session = Session {
                            retired: false,
                            server: &mut server,
                            publications: 0,
                        };
                        (cache.sync_once(&mut session), session.publications)
                    };
                    assert!(publications <= 1);
                    if let Ok(notebook::Synced {
                        edit: Some((id, EditStatus::Published { revision })),
                        ..
                    }) = result
                    {
                        durable(&server, revision);
                        assert!(cache.pending().unwrap().iter().all(|e| e.id != id));
                    }
                }
            }
            _ => {
                let recorded = statuses(cache);
                replicas[actor] = None;
                let cache = Replica::open(&path).unwrap();
                assert_eq!(local(&cache), before);
                assert_eq!(cache.pending().unwrap(), pending);
                assert_eq!(statuses(&cache), recorded);
                replicas[actor] = Some(cache);
            }
        }
        rows(&server.durable);
        rows(&snapshot(replicas[actor].as_ref().unwrap()));
    }
}
