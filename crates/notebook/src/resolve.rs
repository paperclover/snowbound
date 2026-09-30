//! Merging a version of a section file that a file provider kept beside the file, as iCloud
//! Drive keeps a commit that lost to another device's (a conflict version, `NSFileVersion`).
//! The version's changes since the state it last shared with the file replay on the file
//! (`merge.rs`), with conflict pages where they clash, as a queue replays on a changed
//! remote.
//!
//! Each revision the merge writes is named after the version's revision it merged
//! (`merged`). A space whose newest revision the file holds, or holds merged, needs nothing;
//! older ones are where the version and the file last agreed. So a version merged once, by
//! this device or another, merges as nothing the next time, and two devices merging it at
//! once write revisions of the same names, which the next merge of the two finds held.

use crate::{PendingEdit, Result, merge};
use onestore::{
    Arena, ConflictPage, ExGuid, PageCreation, PageEdit, RevisionIndex, Section, Store,
    Transaction,
    op::{Edit, Op, SectionOp, lower_page},
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

/// A version of a section file its file provider keeps beside the file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Names the version to `Remote::version` and `Remote::retire`.
    pub id: String,
    /// The computer or device that saved it (`localizedNameOfSavingComputer`), which labels
    /// the conflict pages it gives.
    pub device: Option<String>,
}

/// What merging a version into the file comes to.
pub(crate) enum Merged {
    /// The file already holds every change the version holds.
    Held,
    /// The revisions bringing the version's changes onto the file.
    Publish(Box<Transaction>),
    /// Another section, which only shares the file's name.
    Foreign,
}

/// The name of the revision a merge writes in a space for the version's revision `rid`, a
/// different one for each `kind` of space it writes.
fn merged(rid: ExGuid, kind: &str) -> ExGuid {
    let digest = Sha256::new()
        .chain_update(b"snowbound merge ")
        .chain_update(kind)
        .chain_update(rid.guid)
        .chain_update(rid.n.to_le_bytes())
        .finalize();
    let mut guid = [0; 16];
    guid.copy_from_slice(&digest[..16]);
    ExGuid { guid, n: 1 }
}

/// Merges `version` into `current`, the file as it stands, `device` naming who saved the
/// version.
pub(crate) fn merge(current: &[u8], version: &[u8], device: &str) -> Result<Merged> {
    let (store, theirs) = (Store::parse(current)?, Store::parse(version)?);
    let (file, stored) = (
        RevisionIndex::parse(&store)?,
        RevisionIndex::parse(&theirs)?,
    );
    if file.root != stored.root {
        return Ok(Merged::Foreign);
    }
    // Per space of the version: where it last agreed with the file, and its newest revision.
    let mut shared = BTreeMap::new();
    let mut heads = BTreeMap::new();
    for (space, revisions) in &stored.spaces {
        let Some(head) = revisions.labels.get(&(ExGuid::default(), 1)).copied() else {
            continue;
        };
        heads.insert(*space, head);
        let ours = file.spaces.get(space);
        // Where the version's revision `rid` meets the file: the file holds it, or holds it
        // merged (a page the file had removed comes back as a copy in a space of its own), or
        // it is the file's revision merged the other way, the state then the base.
        let meets = |rid: ExGuid| {
            let held = ours.is_some_and(|ours| {
                ours.revisions.contains_key(&rid)
                    || ours.revisions.contains_key(&merged(rid, "page"))
            }) || file.spaces.contains_key(&ExGuid {
                guid: merged(rid, "conflict").guid,
                n: 1,
            });
            if held {
                return Some(rid);
            }
            ours?
                .revisions
                .keys()
                .find(|theirs| merged(**theirs, "page") == rid)
                .copied()
        };
        let mut at = Some(head);
        while let Some(rid) = at {
            if let Some(base) = meets(rid) {
                shared.insert(*space, base);
                break;
            }
            at = revisions
                .revisions
                .get(&rid)
                .and_then(|revision| revision.dependency);
        }
    }
    let root = file.root;
    // Without a state of the page list both share, the version's list counts as unchanged.
    let head = |space: &ExGuid| heads.get(space).copied();
    if let std::collections::btree_map::Entry::Vacant(entry) = shared.entry(root) {
        entry.insert(head(&root).ok_or(onestore::Error {
            offset: 0,
            message: "The version has no page list",
        })?);
    }
    let changed: BTreeSet<ExGuid> = heads
        .iter()
        .filter(|(space, head)| shared.get(space) != Some(head))
        .map(|(space, _)| *space)
        .collect();
    let arenas = (Arena::default(), Arena::default(), Arena::default());
    let mut ancestor =
        Section::open_at(&arenas.0, vec![version.to_vec(), current.to_vec()], &shared)?;
    let mut theirs = Section::open(&arenas.1, version.to_vec())?;
    let ours = Section::open(&arenas.2, current.to_vec())?;
    let stored_here = |space: &ExGuid| file.spaces.contains_key(space);

    let listed = theirs.pages()?;
    let before: Vec<ExGuid> = ancestor
        .pages()?
        .into_iter()
        .map(|(space, ..)| space)
        .collect();
    let moved = merge::moved(&mut ancestor, &mut theirs)?;
    let mut ops = Vec::new();
    // From the last page to the first, so that each goes before a page already placed.
    for (at, (space, _, level)) in listed.iter().enumerate().rev() {
        let next = listed.get(at + 1).map(|(next, ..)| *next);
        if !stored_here(space) {
            let page = theirs.page(*space)?;
            let mut creation = PageCreation::new(next, titled(&page), device)?;
            if let (Some(identity), Some(created)) = (page.identity, page.created) {
                creation = creation.keeping(identity, created)?;
            }
            // The page keeps its identities, so that each merge of the version makes it alike.
            let (creation, page) = match space.n {
                1 => (creation.in_space(space.guid)?, page),
                _ => (creation, page.copy()?),
            };
            let space = creation.space();
            ops.push(Op::Section(SectionOp::Import { creation, page }));
            if *level > 1 {
                ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::set_level(
                    space, *level,
                )?])));
            }
        } else if moved.contains(space) {
            ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                *space, next, *level,
            )?])));
        }
    }
    let kept: BTreeSet<ExGuid> = listed.iter().map(|(space, ..)| *space).collect();
    let conflicts =
        |section: &mut Section<'_>| -> Result<BTreeMap<ExGuid, (ExGuid, ConflictPage)>> {
            Ok(section
                .conflicts()?
                .into_iter()
                .flat_map(|(of, pages)| pages.into_iter().map(move |page| (page.space, (of, page))))
                .collect())
        };
    let (was, now) = (conflicts(&mut ancestor)?, conflicts(&mut theirs)?);
    let removed: Vec<ExGuid> = before
        .iter()
        .filter(|space| !kept.contains(space))
        .chain(was.keys().filter(|space| !now.contains_key(space)))
        .copied()
        .collect();
    if !removed.is_empty() {
        ops.push(Op::Section(SectionOp::Delete(removed)));
    }
    // Pages both hold with no state in common, or that do not lower to ops: the version's is
    // kept beside the file's.
    let mut unrelated = BTreeSet::new();
    for (space, ..) in &listed {
        if !changed.contains(space) || !stored_here(space) {
            continue;
        }
        let page = theirs.page(*space)?;
        let lowered = match shared.get(space) {
            Some(_) => lower_page(&ancestor.page(*space)?, &page).ok(),
            None if ours.page(*space).ok().as_ref() == Some(&page) => Some(Vec::new()),
            None => None,
        };
        match lowered {
            Some(lowered) => {
                ops.extend(lowered.into_iter().map(|op| Op::Page { space: *space, op }))
            }
            None => {
                unrelated.insert(*space);
            }
        }
    }
    for (space, (of, conflict)) in now {
        if stored_here(&space) || was.contains_key(&space) {
            continue;
        }
        let page = theirs.page(space)?;
        let user = Some(conflict.user).filter(|user| !user.is_empty());
        ops.push(Op::Section(SectionOp::Conflict {
            of,
            creation: PageCreation::new(None, titled(&page), user.as_deref().unwrap_or(device))?
                .in_space(space.guid)?,
            page,
            objects: conflict.objects,
        }));
    }
    let edits = [PendingEdit {
        id: 0,
        author: device.to_owned(),
        edit: Edit {
            at: crate::now(),
            ops,
        },
    }];

    // As a queue rebases (`working::rebase`): pages the file already holds as the version
    // leaves them are done; the rest that cannot replay become conflict pages.
    let mut converged = BTreeSet::new();
    loop {
        let arena = Arena::default();
        let mut new = Section::open(&arena, current.to_vec())?;
        let replayed = merge::rebase(&mut ancestor, &mut new, &edits, &converged)?;
        let mut clashes: BTreeMap<ExGuid, BTreeSet<ExGuid>> = replayed
            .conflicts
            .into_iter()
            .map(|(space, (_, objects))| (space, objects))
            .collect();
        let settled: Vec<ExGuid> = clashes
            .keys()
            .filter(|space| {
                matches!((ours.page(**space), theirs.page(**space)), (Ok(a), Ok(b)) if a == b)
            })
            .copied()
            .collect();
        if !settled.is_empty() {
            converged.extend(settled);
            continue;
        }
        for space in &unrelated {
            clashes.entry(*space).or_default();
        }
        let mut names: BTreeMap<ExGuid, ExGuid> = changed
            .iter()
            .filter_map(|space| Some((*space, merged(head(space)?, "page"))))
            .collect();
        for (space, objects) in &clashes {
            let Ok(page) = theirs.page(*space) else {
                continue;
            };
            let Some(rid) = head(space) else {
                continue;
            };
            // Named after the version's page, so that every merge of it makes the same page.
            let guid = merged(rid, "conflict").guid;
            merge::conflict_page(
                &mut new,
                &mut theirs,
                &replayed.moved,
                *space,
                &page,
                device,
                objects,
                crate::now(),
                Some(guid),
            )?;
            names.insert(ExGuid { guid, n: 1 }, merged(rid, "page"));
        }
        return Ok(match new.seal_as(&names)? {
            Some(transaction) => Merged::Publish(Box::new(transaction)),
            None => Merged::Held,
        });
    }
}

/// A page's title, where it has one, as a page made for it takes.
fn titled(page: &onestore::page::Page) -> Option<&str> {
    page.objects
        .iter()
        .any(|object| matches!(object, onestore::page::PageObject::Title(_)))
        .then_some(page.title.as_str())
}
