//! Undo and Redo across pages, sections and notebooks (`arc/canvas.md`). Each page keeps
//! its own history in its editor, kept while the window is open as OneNote 2010 keeps it; the
//! window keeps what was done to pages and sections between those edits, so Undo takes back
//! whichever came last where the user is. Every step taken back is an edit of its own.

use crate::{Command, Library, Loaded, State, UserEvent};
use canvas::editor::CanvasEditor;
use notebook::{Replica, discover::Folder};
use onestore::{
    ExGuid, PageCreation, PageEdit,
    op::{Edit, Op, SectionOp},
    page::{Page, PageObject},
};
use std::{error::Error, sync::Arc};

/// Pages whose editors are kept with their history once left, at most.
const KEPT: usize = 50;

#[derive(Default)]
pub struct Timeline {
    undo: Vec<Step>,
    redo: Vec<Step>,
    /// Editors of pages left holding history, the longest left first.
    parked: Vec<(ExGuid, CanvasEditor)>,
    /// Actions on their way, while which Undo and Redo wait.
    busy: usize,
}

enum Step {
    /// One step of a page's own history, which its editor holds.
    Edit(ExGuid),
    Action(Action),
}

impl Step {
    fn edits(&self, page: ExGuid) -> bool {
        matches!(self, Step::Edit(edited) if *edited == page)
    }
}

/// A change to pages or sections and where it was made: a notebook, by location, and
/// for a change to pages, the section by file identity. Undo reaches it there.
pub struct Action {
    notebook: String,
    section: Option<[u8; 16]>,
    change: Change,
}

/// What an action does when taken; doing one gives the action that takes it back.
pub enum Change {
    /// Takes pages out of the section, to the recycle bin where the notebook has one. Pages
    /// `created` go only while they hold nothing, and never to the bin. Shows `show` after,
    /// else the page after the first one taken, or before it.
    Delete {
        pages: Vec<ExGuid>,
        show: Option<ExGuid>,
        created: bool,
    },
    /// Puts pages back where they were, taking them out of the recycle bin unless `created`,
    /// and takes out `added`, the page a section left without pages gained, while it holds
    /// nothing.
    Restore {
        pages: Vec<Kept>,
        added: Option<ExGuid>,
        created: bool,
    },
    /// Puts the pages `moved` back as `order` lists the section's pages, with their levels.
    Arrange {
        order: Vec<(ExGuid, u32)>,
        moved: Vec<ExGuid>,
    },
    /// Titles a page `to` while its title is `from`.
    Retitle {
        page: ExGuid,
        from: String,
        to: String,
    },
    /// Moves a page to the end of the section `target` names, as a drop on its tab does.
    Send { page: ExGuid, target: [u8; 16] },
    /// Brings a page sent to `target` back, before `before` at `level`, taking out `added`
    /// while it holds nothing.
    Return {
        page: ExGuid,
        target: [u8; 16],
        before: Option<ExGuid>,
        level: u32,
        added: Option<ExGuid>,
    },
    /// Names a section or group `to` while it is named `from`.
    Rename {
        entry: [u8; 16],
        from: String,
        to: String,
    },
    /// Puts a section or group, `entry`, in the folder `folder` names (the notebook's own
    /// without), which then takes `order`.
    Place {
        entry: Option<[u8; 16]>,
        folder: Option<[u8; 16]>,
        order: Vec<[u8; 16]>,
    },
}

/// A page taken out of its section, and where it was: before `before` at `level`.
pub struct Kept {
    page: Page,
    before: Option<ExGuid>,
    level: u32,
}

/// Where the user is: the open section's notebook, by location, and the section.
pub struct Here {
    notebook: String,
    section: [u8; 16],
}

impl Action {
    fn applies(&self, here: &Here) -> bool {
        self.notebook == here.notebook && self.section.is_none_or(|section| section == here.section)
    }
}

/// What Undo or Redo takes next.
pub enum Next {
    /// The open page's last edit, or the last undone.
    Edit,
    Action(Action),
    /// The page a created page Undo would take out still holds edits made since, so
    /// Undo shows it, where those go back first.
    Show(ExGuid),
}

impl Timeline {
    /// Notes `action`, just done; what Redo held of actions goes.
    pub fn record(&mut self, action: Action) {
        self.undo.push(Step::Action(action));
        self.redo.retain(|step| matches!(step, Step::Edit(_)));
    }

    /// Notes `action`, which takes back one the user just did (`taken` none), or Undo, or
    /// `taken` Redo, just took.
    fn note(&mut self, taken: Option<bool>, action: Action) {
        match taken {
            None => self.record(action),
            Some(true) => self.undo.push(Step::Action(action)),
            Some(false) => self.redo.push(Step::Action(action)),
        }
    }

    /// Follows page `page`'s history, `depth` steps of Undo and Redo, after an edit or
    /// before one is taken: steps it gained are the newest; `run`, a run of typing that
    /// went on, makes its last step the newest.
    pub fn edited(&mut self, page: ExGuid, [undo, redo]: [usize; 2], run: bool) {
        let count = |steps: &[Step]| steps.iter().filter(|step| step.edits(page)).count();
        let (had, had_redo) = (count(&self.undo), count(&self.redo));
        if undo < had {
            drop_newest(&mut self.undo, page, had - undo);
        } else if undo > had || run {
            // A run of typing that went on makes its step the newest.
            if undo == had
                && let Some(at) = self.undo.iter().rposition(|step| step.edits(page))
            {
                let step = self.undo.remove(at);
                self.undo.push(step);
            }
            self.undo.extend((had..undo).map(|_| Step::Edit(page)));
            // A new edit ends what actions Redo held.
            self.redo.retain(|step| matches!(step, Step::Edit(_)));
        }
        if redo < had_redo {
            drop_newest(&mut self.redo, page, had_redo - redo);
        } else {
            self.redo.extend((had_redo..redo).map(|_| Step::Edit(page)));
        }
    }

    /// What Undo, or `redo` Redo, takes next on page `page`, its history `depth` steps deep,
    /// with `here` open: the newer of the page's own step and the last action made there.
    /// An action is taken off the timeline; an edit stays until `stepped`.
    pub fn next(
        &mut self,
        redo: bool,
        page: ExGuid,
        depth: [usize; 2],
        here: &Here,
    ) -> Option<Next> {
        if self.busy > 0 {
            return None;
        }
        self.edited(page, depth, false);
        let steps = if redo { &mut self.redo } else { &mut self.undo };
        let at = steps.iter().rposition(|step| match step {
            Step::Edit(edited) => *edited == page,
            Step::Action(action) => action.applies(here),
        })?;
        match &steps[at] {
            Step::Edit(_) => return Some(Next::Edit),
            Step::Action(Action {
                change:
                    Change::Delete {
                        pages,
                        created: true,
                        ..
                    },
                ..
            }) if !redo => {
                let edited = pages
                    .iter()
                    .find(|page| steps.iter().any(|step| step.edits(**page)));
                if let Some(edited) = edited {
                    return Some(Next::Show(*edited));
                }
            }
            Step::Action(_) => {}
        }
        match steps.remove(at) {
            Step::Action(action) => Some(Next::Action(action)),
            Step::Edit(_) => unreachable!("an edit is not taken off"),
        }
    }

    /// Notes that Undo, or `redo` Redo, took page `page`'s step.
    pub fn stepped(&mut self, page: ExGuid, redo: bool) {
        let (from, to) = if redo {
            (&mut self.redo, &mut self.undo)
        } else {
            (&mut self.undo, &mut self.redo)
        };
        if let Some(at) = from.iter().rposition(|step| step.edits(page)) {
            to.push(from.remove(at));
        }
    }

    /// Whether Undo, or `redo` Redo, has an action to take `here`.
    pub fn reaches(&self, redo: bool, here: &Here) -> bool {
        let steps = if redo { &self.redo } else { &self.undo };
        self.busy == 0
            && steps
                .iter()
                .any(|step| matches!(step, Step::Action(action) if action.applies(here)))
    }

    /// Keeps the editor of page `page`, just left, for its history.
    pub fn park(&mut self, page: ExGuid, editor: CanvasEditor) {
        self.parked.retain(|(parked, _)| *parked != page);
        if editor.history_depth() == [0, 0] || editor.marked_range().is_some() {
            return self.forget(&[page]);
        }
        self.parked.push((page, editor));
        if self.parked.len() > KEPT {
            let (oldest, _) = self.parked.remove(0);
            self.forget(&[oldest]);
        }
    }

    /// The editor page `page` was left with, which takes up its history again.
    pub fn resume(&mut self, page: ExGuid) -> Option<CanvasEditor> {
        let at = self.parked.iter().position(|(parked, _)| *parked == page)?;
        Some(self.parked.remove(at).1)
    }

    /// Drops the histories of pages gone.
    pub fn forget(&mut self, pages: &[ExGuid]) {
        self.parked.retain(|(parked, _)| !pages.contains(parked));
        for steps in [&mut self.undo, &mut self.redo] {
            steps.retain(|step| !matches!(step, Step::Edit(page) if pages.contains(page)));
        }
    }

    /// Drops the actions of the notebook at `location`, closed.
    pub fn close(&mut self, location: &str) {
        for steps in [&mut self.undo, &mut self.redo] {
            steps.retain(
                |step| !matches!(step, Step::Action(action) if action.notebook == location),
            );
        }
    }
}

/// Removes the `count` newest steps of page `page`.
fn drop_newest(steps: &mut Vec<Step>, page: ExGuid, count: usize) {
    for _ in 0..count {
        if let Some(at) = steps.iter().rposition(|step| step.edits(page)) {
            steps.remove(at);
        }
    }
}

/// Whether `page` holds nothing but its title, untitled.
fn blank(page: &Page) -> bool {
    page.title.trim().is_empty()
        && page
            .objects
            .iter()
            .all(|object| matches!(object, PageObject::Title(_)))
}

/// The page shown once `gone` leave the section listing `listed`: `prefer` if it stays,
/// else the page after the first one gone, or before it, as OneNote 2010 shows.
fn neighbor(
    listed: &[(ExGuid, String, u32)],
    gone: &[ExGuid],
    prefer: Option<ExGuid>,
) -> Option<ExGuid> {
    let stays = |space: &ExGuid| !gone.contains(space) && listed.iter().any(|(s, ..)| s == space);
    if let Some(prefer) = prefer.filter(stays) {
        return Some(prefer);
    }
    let at = listed
        .iter()
        .position(|(space, ..)| gone.contains(space))
        .unwrap_or_default();
    listed[at..]
        .iter()
        .chain(listed[..at].iter().rev())
        .map(|(space, ..)| *space)
        .find(stays)
}

/// The space of the page an import op creates.
fn imported(op: &Op) -> ExGuid {
    match op {
        Op::Section(SectionOp::Import { creation, .. }) => creation.space(),
        _ => unreachable!("notebook::session::moved imports"),
    }
}

/// What a change to pages works with: the open section, its notebook, who changes it, and
/// a page to give it should it be left without pages.
struct Site {
    replica: Arc<Replica>,
    library: Arc<Library>,
    author: String,
    fresh: PageCreation,
}

/// A change to pages done: the change taking it back, the page to show, and pages gone.
struct Applied {
    undo: Change,
    show: ExGuid,
    gone: Vec<ExGuid>,
}

impl Site {
    fn apply(&self, ops: Vec<Op>) -> Result<(), Box<dyn Error>> {
        self.replica.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops,
            },
        )?;
        Ok(())
    }

    /// The catalog path of the section `identity` names.
    fn section_path(&self, identity: [u8; 16]) -> Option<String> {
        entry_of(self.library.catalog()?, identity).map(|(_, path)| path)
    }

    /// Does `change` to the section's pages, `shown` the page open; `None` where what it
    /// changes is gone or holds what it did not make, which it leaves alone.
    fn change(&self, change: Change, shown: ExGuid) -> Result<Option<Applied>, Box<dyn Error>> {
        let listed = self.replica.pages()?;
        let at = |space: ExGuid| listed.iter().position(|(listed, ..)| *listed == space);
        let binned = matches!(self.library.notebook, Ok(Some(_)));
        Ok(Some(match change {
            Change::Delete {
                pages,
                show,
                created,
            } => {
                let pages: Vec<ExGuid> = pages
                    .into_iter()
                    .filter(|page| at(*page).is_some())
                    .collect();
                let read: Vec<Page> = pages
                    .iter()
                    .map(|page| self.replica.page(*page))
                    .collect::<Result<_, _>>()?;
                if pages.is_empty() || created && !read.iter().all(blank) {
                    return Ok(None);
                }
                let kept: Vec<Kept> = pages
                    .iter()
                    .zip(read)
                    .map(|(space, page)| {
                        let at = at(*space).expect("listed");
                        Kept {
                            page,
                            before: listed[at + 1..]
                                .iter()
                                .map(|(space, ..)| *space)
                                .find(|space| !pages.contains(space)),
                            level: listed[at].2,
                        }
                    })
                    .collect();
                let mut ops = vec![Op::Section(SectionOp::Delete(pages.clone()))];
                let (show, added) = match neighbor(&listed, &pages, show) {
                    Some(show) => (show, None),
                    None => {
                        let space = self.fresh.space();
                        ops.push(Op::Section(SectionOp::Create(self.fresh.clone())));
                        (space, Some(space))
                    }
                };
                if !created && binned {
                    let pages: Vec<Page> = kept.iter().map(|kept| kept.page.clone()).collect();
                    self.library.reopen()?.recycle_pages(&pages, &self.author)?;
                }
                self.apply(ops)?;
                Applied {
                    undo: Change::Restore {
                        pages: kept,
                        added,
                        created,
                    },
                    show,
                    gone: pages,
                }
            }
            Change::Restore {
                pages,
                added,
                created,
            } => {
                // A page someone put back already, listed by its identity, stays as it is.
                let identity = |space: ExGuid| self.replica.page(space).ok()?.identity;
                let mut back = Vec::new();
                for kept in pages {
                    let listed_again = kept.page.identity.is_some()
                        && listed.iter().any(|(space, title, _)| {
                            *title == kept.page.title && identity(*space) == kept.page.identity
                        });
                    if !listed_again {
                        back.push(kept);
                    }
                }
                if back.is_empty() {
                    return Ok(None);
                }
                let mut ops = Vec::new();
                let mut spaces = Vec::new();
                for kept in &back {
                    let import = notebook::session::moved(&kept.page, &self.author)?;
                    let space = imported(&import);
                    let before = kept.before.filter(|before| at(*before).is_some());
                    ops.push(import);
                    ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                        space, before, kept.level,
                    )?])));
                    spaces.push(space);
                }
                let gone: Vec<ExGuid> = added
                    .filter(|added| {
                        at(*added).is_some() && self.replica.page(*added).is_ok_and(|p| blank(&p))
                    })
                    .into_iter()
                    .collect();
                if !gone.is_empty() {
                    ops.push(Op::Section(SectionOp::Delete(gone.clone())));
                }
                self.apply(ops)?;
                if !created && binned {
                    let identities: Vec<[u8; 16]> =
                        back.iter().filter_map(|kept| kept.page.identity).collect();
                    self.library.reopen()?.unrecycle_pages(&identities)?;
                }
                Applied {
                    undo: Change::Delete {
                        pages: spaces.clone(),
                        show: Some(shown),
                        created,
                    },
                    show: spaces[0],
                    gone,
                }
            }
            Change::Arrange { order, moved } => {
                let moving = |space: &ExGuid| moved.contains(space) && at(*space).is_some();
                let mut edits = Vec::new();
                for (index, (space, level)) in order.iter().enumerate() {
                    if !moving(space) {
                        continue;
                    }
                    let before = order[index + 1..]
                        .iter()
                        .map(|(space, _)| *space)
                        .find(|space| !moved.contains(space) && at(*space).is_some());
                    edits.push(PageEdit::move_to(*space, before, *level)?);
                }
                let Some(show) = edits.first().map(PageEdit::space) else {
                    return Ok(None);
                };
                self.apply(vec![Op::Section(SectionOp::Pages(edits))])?;
                Applied {
                    undo: Change::Arrange {
                        order: listed
                            .iter()
                            .map(|(space, _, level)| (*space, *level))
                            .collect(),
                        moved,
                    },
                    show,
                    gone: Vec::new(),
                }
            }
            Change::Retitle { page, from, to } => {
                if at(page).is_none_or(|at| listed[at].1.trim() != from.trim()) {
                    return Ok(None);
                }
                let op = crate::rename::retitled(&self.replica.page(page)?, page, to.clone())?;
                self.apply(vec![op])?;
                Applied {
                    undo: Change::Retitle {
                        page,
                        from: to,
                        to: from,
                    },
                    show: page,
                    gone: Vec::new(),
                }
            }
            Change::Send { page, target } => {
                let (Some(from), Some(path)) = (at(page), self.section_path(target)) else {
                    return Ok(None);
                };
                let import = notebook::session::moved(&self.replica.page(page)?, &self.author)?;
                let arrived = imported(&import);
                let before = listed[from + 1..].first().map(|(space, ..)| *space);
                let level = listed[from].2;
                let mut ops = vec![Op::Section(SectionOp::Delete(vec![page]))];
                let (show, added) = match neighbor(&listed, &[page], None) {
                    Some(show) => (show, None),
                    None => {
                        let space = self.fresh.space();
                        ops.push(Op::Section(SectionOp::Create(self.fresh.clone())));
                        (space, Some(space))
                    }
                };
                let section = self.library.open(&path, || {})?;
                section.replica().apply(
                    &self.author,
                    Edit {
                        at: crate::filetime(),
                        ops: vec![import],
                    },
                )?;
                section.close()?;
                self.apply(ops)?;
                Applied {
                    undo: Change::Return {
                        page: arrived,
                        target,
                        before,
                        level,
                        added,
                    },
                    show,
                    gone: vec![page],
                }
            }
            Change::Return {
                page,
                target,
                before,
                level,
                added,
            } => {
                let Some(path) = self.section_path(target) else {
                    return Ok(None);
                };
                let section = self.library.open(&path, || {})?;
                let sent = match section.pages()?.iter().any(|(listed, ..)| *listed == page) {
                    true => Some(section.page(page)?),
                    false => None,
                };
                let Some(sent) = sent else {
                    section.close()?;
                    return Ok(None);
                };
                let import = notebook::session::moved(&sent, &self.author)?;
                let back = imported(&import);
                let mut ops = vec![
                    import,
                    Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                        back,
                        before.filter(|before| at(*before).is_some()),
                        level,
                    )?])),
                ];
                let gone: Vec<ExGuid> = added
                    .filter(|added| {
                        at(*added).is_some() && self.replica.page(*added).is_ok_and(|p| blank(&p))
                    })
                    .into_iter()
                    .collect();
                if !gone.is_empty() {
                    ops.push(Op::Section(SectionOp::Delete(gone.clone())));
                }
                self.apply(ops)?;
                section.replica().apply(
                    &self.author,
                    Edit {
                        at: crate::filetime(),
                        ops: vec![Op::Section(SectionOp::Delete(vec![page]))],
                    },
                )?;
                section.close()?;
                Applied {
                    undo: Change::Send { page: back, target },
                    show: back,
                    gone,
                }
            }
            Change::Rename { .. } | Change::Place { .. } => {
                unreachable!("a section's own change is the notebook's")
            }
        }))
    }
}

/// The identity of each section and group in `folder`, in order, with its catalog path;
/// the recycle bin is left out.
fn entries(folder: &Folder) -> Vec<([u8; 16], String)> {
    folder
        .sections
        .iter()
        .map(|section| (section.file_id, section.path.clone()))
        .chain(folder.groups.iter().filter_map(|group| {
            let toc = group.toc.as_ref()?;
            (!crate::library::recycle_bin(&group.path)).then(|| (toc.file_id, group.path.clone()))
        }))
        .collect()
}

/// The identity of a group's folder; none for the notebook's own.
fn folder_id(folder: &Folder) -> Option<[u8; 16]> {
    let toc = folder.toc.as_ref()?;
    (!folder.path.is_empty()).then_some(toc.file_id)
}

/// Every folder of `catalog`, the notebook's own first.
fn folders(catalog: &Folder) -> Vec<&Folder> {
    let mut all = vec![catalog];
    let mut at = 0;
    while let Some(folder) = all.get(at) {
        all.extend(&folder.groups);
        at += 1;
    }
    all
}

/// The folder `identity` names, the notebook's own without one.
fn folder_of(catalog: &Folder, identity: Option<[u8; 16]>) -> Option<&Folder> {
    match identity {
        Some(_) => folders(catalog)
            .into_iter()
            .find(|folder| folder_id(folder) == identity),
        None => Some(catalog),
    }
}

/// The section or group `identity` names: its folder and catalog path.
fn entry_of(catalog: &Folder, identity: [u8; 16]) -> Option<(&Folder, String)> {
    folders(catalog).into_iter().find_map(|folder| {
        let (_, path) = entries(folder)
            .into_iter()
            .find(|(id, _)| *id == identity)?;
        Some((folder, path))
    })
}

/// The identity of the section or group at catalog `path`.
fn identity_at(catalog: &Folder, path: &str) -> Option<[u8; 16]> {
    folders(catalog).into_iter().find_map(|folder| {
        let (id, _) = entries(folder).into_iter().find(|(_, at)| at == path)?;
        Some(id)
    })
}

/// A section or group's name, as its catalog path ends.
fn name_of(path: &str) -> &str {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".one").unwrap_or(name)
}

/// The action taking back `change` to `library`'s sections and groups, which is about to be
/// made; none for changes Undo does not take back, as OneNote 2010 takes back none.
pub fn structure_undo(library: &Library, change: &crate::manage::Structure) -> Option<Action> {
    use crate::manage::Structure;
    let catalog = library.catalog()?;
    let change = match change {
        Structure::Rename { path, name } => Change::Rename {
            entry: identity_at(catalog, path)?,
            from: name.clone(),
            to: name_of(path).to_owned(),
        },
        Structure::Move { path, .. } => {
            let entry = identity_at(catalog, path)?;
            let (folder, _) = entry_of(catalog, entry)?;
            Change::Place {
                entry: Some(entry),
                folder: folder_id(folder),
                order: entries(folder).into_iter().map(|(id, _)| id).collect(),
            }
        }
        Structure::Reorder { folder, .. } => {
            let folder = folders(catalog)
                .into_iter()
                .find(|listed| listed.path == *folder)?;
            Change::Place {
                entry: None,
                folder: folder_id(folder),
                order: entries(folder).into_iter().map(|(id, _)| id).collect(),
            }
        }
        _ => return None,
    };
    Some(Action {
        notebook: library.location.clone(),
        section: None,
        change,
    })
}

/// The structure change carrying out a section or group's `change` in `library` now, with
/// the action taking it back; none where what it changes is gone or was changed since.
fn restructuring(library: &Library, change: &Change) -> Option<(crate::manage::Structure, Action)> {
    use crate::manage::Structure;
    let catalog = library.catalog()?;
    let (structure, undo) = match change {
        Change::Rename { entry, from, to } => {
            let (_, path) = entry_of(catalog, *entry)?;
            if name_of(&path) != from {
                return None;
            }
            (
                Structure::Rename {
                    path,
                    name: to.clone(),
                },
                Change::Rename {
                    entry: *entry,
                    from: to.clone(),
                    to: from.clone(),
                },
            )
        }
        Change::Place {
            entry,
            folder,
            order,
        } => {
            let target = folder_of(catalog, *folder)?;
            let placed = match entry {
                Some(entry) => Some(entry_of(catalog, *entry)?),
                None => None,
            };
            // The folder the entry is in now, which the action taking this back reorders.
            let now = placed.as_ref().map_or(target, |(folder, _)| *folder);
            let undo = Change::Place {
                entry: *entry,
                folder: folder_id(now),
                order: entries(now).into_iter().map(|(id, _)| id).collect(),
            };
            let moving = placed.filter(|(from, _)| from.path != target.path);
            let arrived = moving.as_ref().map(|(_, path)| {
                let name = path.rsplit('/').next().unwrap_or(path);
                match target.path.as_str() {
                    "" => name.to_owned(),
                    folder => format!("{folder}/{name}"),
                }
            });
            let current = entries(target);
            let paths: Vec<String> = order
                .iter()
                .filter_map(|id| match &arrived {
                    Some(arrived) if Some(*id) == *entry => Some(arrived.clone()),
                    _ => current
                        .iter()
                        .find(|(at, _)| at == id)
                        .map(|(_, path)| path.clone()),
                })
                .collect();
            let structure = match moving {
                Some((_, path)) => Structure::Return {
                    path,
                    folder: target.path.clone(),
                    paths,
                },
                None => Structure::Reorder {
                    folder: target.path.clone(),
                    paths,
                },
            };
            (structure, undo)
        }
        _ => return None,
    };
    Some((
        structure,
        Action {
            notebook: library.location.clone(),
            section: None,
            change: undo,
        },
    ))
}

impl State {
    /// The open section and its notebook, where Undo reaches. A section opened on its own
    /// is its notebook's only one.
    fn here(&self) -> Option<Here> {
        let session = self.session.as_ref()?;
        let path = &session.tabs.get(session.tab)?.path;
        Some(Here {
            notebook: session.library.location.clone(),
            section: session.library.section_identity(path).unwrap_or_default(),
        })
    }

    /// Whether Undo, or `redo` Redo, has something to take.
    pub(crate) fn can_step(&self, redo: bool) -> bool {
        let editor = &self.view.editor;
        (if redo {
            editor.can_redo()
        } else {
            editor.can_undo()
        }) || self
            .here()
            .is_some_and(|here| self.undo.reaches(redo, &here))
    }

    /// Undo, or `redo` Redo: the open page's last edit or the last action made where the
    /// user is, whichever came last.
    pub(crate) fn step(&mut self, redo: bool) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let depth = self.view.editor.history_depth();
        // A page kept in no notebook has only its own history.
        let shown = self.session.as_ref().map(|session| session.space);
        let next = match (self.here(), shown) {
            (Some(here), Some(page)) => self.undo.next(redo, page, depth, &here),
            _ => Some(Next::Edit),
        };
        match next {
            None => {}
            Some(Next::Edit) => {
                let response = self.view.undo(redo)?;
                self.respond(response);
                if let Some(page) = shown
                    && self.view.editor.history_depth() != depth
                {
                    self.undo.stepped(page, redo);
                }
            }
            Some(Next::Show(page)) => self.commands.push(Command::OpenPage(page)),
            Some(Next::Action(action)) => self.perform(action, Some(redo))?,
        }
        Ok(())
    }

    /// Does `action`: one the user asked for, or that Undo, or `taken` Redo, took. What
    /// takes it back goes on the timeline.
    pub(crate) fn perform(
        &mut self,
        action: Action,
        taken: Option<bool>,
    ) -> Result<(), Box<dyn Error>> {
        let Action {
            notebook,
            section,
            change,
        } = action;
        let Some(section) = section else {
            let library = self
                .notebooks
                .iter()
                .find(|library| library.location == notebook)
                .cloned()
                .ok_or("That notebook is closed")?;
            let Some((structure, undo)) = restructuring(&library, &change) else {
                return match taken {
                    Some(redo) => self.step(redo),
                    None => Ok(()),
                };
            };
            self.undo.note(taken, undo);
            self.restructure(library, structure);
            return Ok(());
        };
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        let site = Site {
            replica: Arc::clone(session.section.replica()),
            library: Arc::clone(&session.library),
            author: self.author.clone(),
            fresh: self.dated_page(None)?,
        };
        let shown = session.space;
        let proxy = self.proxy.clone();
        self.undo.busy += 1;
        self.load(move || {
            let applied = site.change(change, shown);
            let show = match &applied {
                Ok(Some(applied)) => applied.show,
                _ => shown,
            };
            let failed = applied.as_ref().err().map(ToString::to_string);
            let (stale, applied) = (failed.is_none(), applied.ok().flatten());
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |state: &mut State| {
                state.undo.busy -= 1;
                let Some(applied) = applied else {
                    // What it would change is gone: Undo goes on to the step before.
                    return match taken {
                        Some(redo) if stale => state.step(redo),
                        _ => Ok(()),
                    };
                };
                state.undo.forget(&applied.gone);
                let undo = Action {
                    notebook,
                    section: Some(section),
                    change: applied.undo,
                };
                state.undo.note(taken, undo);
                state.edited(Vec::new());
                Ok(())
            })));
            if let Some(failed) = failed {
                return Err(failed.into());
            }
            Ok(Loaded::Page(show, site.replica.page(show)?))
        });
        Ok(())
    }

    /// An action the user takes on the open section's pages: `change`.
    pub(crate) fn change_pages(&mut self, change: Change) -> Result<(), Box<dyn Error>> {
        let here = self.here().ok_or("No section is open")?;
        self.perform(
            Action {
                notebook: here.notebook,
                section: Some(here.section),
                change,
            },
            None,
        )
    }

    /// Notes `change`, just made on the open section's pages, for Undo to take back.
    pub(crate) fn made(&mut self, change: Change) {
        if let Some(here) = self.here() {
            self.undo.record(Action {
                notebook: here.notebook,
                section: Some(here.section),
                change,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notebook::session::Notebook;

    fn page(n: u32) -> ExGuid {
        ExGuid { guid: [9; 16], n }
    }

    const SECTION: [u8; 16] = [1; 16];

    fn here() -> Here {
        Here {
            notebook: "/n".into(),
            section: SECTION,
        }
    }

    fn created(space: ExGuid) -> Action {
        Action {
            notebook: "/n".into(),
            section: Some(SECTION),
            change: Change::Delete {
                pages: vec![space],
                show: None,
                created: true,
            },
        }
    }

    fn is_action(next: Option<Next>) -> bool {
        matches!(next, Some(Next::Action(_)))
    }

    fn is_edit(next: Option<Next>) -> bool {
        matches!(next, Some(Next::Edit))
    }

    /// Cmd+N, a title typed, then Undo: the title goes, then the page.
    #[test]
    fn undo_takes_back_a_new_page_after_its_title() {
        let mut timeline = Timeline::default();
        let (old, new) = (page(1), page(2));
        timeline.edited(old, [3, 0], false);
        timeline.record(created(new));
        timeline.edited(new, [1, 0], false);
        assert!(is_edit(timeline.next(false, new, [1, 0], &here())));
        timeline.stepped(new, false);
        assert!(is_action(timeline.next(false, new, [0, 1], &here())));
        // The page shown before keeps its own history.
        assert!(is_edit(timeline.next(false, old, [3, 0], &here())));
    }

    /// The newer of the page's own step and the last action goes first, as OneNote 2010
    /// takes back a page deleted after typing on the page it then shows.
    #[test]
    fn the_newer_of_page_and_action_goes_first() {
        let mut timeline = Timeline::default();
        let shown = page(1);
        timeline.edited(shown, [1, 0], false);
        timeline.record(created(page(5)));
        assert!(is_action(timeline.next(false, shown, [1, 0], &here())));
        timeline.edited(shown, [2, 0], false);
        timeline.record(created(page(6)));
        // Typing that goes on a run started before the action is newer than it.
        timeline.edited(shown, [2, 0], true);
        assert!(is_edit(timeline.next(false, shown, [2, 0], &here())));
    }

    /// Another page's edits are its own; a new page holding edits is shown before it goes.
    #[test]
    fn a_created_page_with_edits_is_shown_first() {
        let mut timeline = Timeline::default();
        let (other, new) = (page(1), page(2));
        timeline.record(created(new));
        timeline.edited(new, [2, 0], false);
        assert!(matches!(
            timeline.next(false, other, [0, 0], &here()),
            Some(Next::Show(shown)) if shown == new
        ));
        // Its edits taken back there, it goes.
        timeline.stepped(new, false);
        timeline.stepped(new, false);
        assert!(is_action(timeline.next(false, other, [0, 0], &here())));
    }

    /// Actions are reached where they were made: a page change in its section, a section
    /// change anywhere in its notebook.
    #[test]
    fn actions_are_reached_where_they_were_made() {
        let mut timeline = Timeline::default();
        timeline.record(created(page(2)));
        timeline.record(Action {
            notebook: "/n".into(),
            section: None,
            change: Change::Rename {
                entry: [3; 16],
                from: "B".into(),
                to: "A".into(),
            },
        });
        let elsewhere = Here {
            notebook: "/n".into(),
            section: [2; 16],
        };
        let other = Here {
            notebook: "/m".into(),
            section: SECTION,
        };
        assert!(!timeline.reaches(false, &other));
        assert!(matches!(
            timeline.next(false, page(7), [0, 0], &elsewhere),
            Some(Next::Action(Action {
                change: Change::Rename { .. },
                ..
            }))
        ));
        assert!(timeline.next(false, page(7), [0, 0], &elsewhere).is_none());
        assert!(is_action(timeline.next(false, page(7), [0, 0], &here())));
        timeline.record(created(page(3)));
        timeline.close("/n");
        assert!(!timeline.reaches(false, &here()));
    }

    /// Redo takes back the last undone first; a new edit ends what actions it held, and
    /// waits while an action is on its way.
    #[test]
    fn redo_mirrors_undo_and_new_edits_end_it() {
        let mut timeline = Timeline::default();
        let shown = page(1);
        timeline.record(created(page(2)));
        let Some(Next::Action(action)) = timeline.next(false, shown, [0, 0], &here()) else {
            panic!()
        };
        timeline.note(Some(false), action);
        timeline.edited(shown, [1, 0], false);
        assert!(is_edit(timeline.next(false, shown, [1, 0], &here())));
        timeline.stepped(shown, false);
        assert!(is_edit(timeline.next(true, shown, [0, 1], &here())));
        // The undone action went with the edit made after it.
        timeline.stepped(shown, true);
        timeline.stepped(shown, false);
        assert!(!timeline.reaches(true, &here()));
        timeline.record(created(page(3)));
        timeline.busy = 1;
        assert!(timeline.next(false, shown, [0, 1], &here()).is_none());
    }

    const AUTHOR: &str = "Rust Author";

    fn dated() -> PageCreation {
        PageCreation::new(None, Some(""), AUTHOR)
            .unwrap()
            .dated("Wednesday, September 30, 2026", "7:04 PM")
            .unwrap()
    }

    /// A notebook as the app makes one, its first section holding pages titled `titles`
    /// and open, beside a section "B"; deleted when dropped.
    struct Fixture {
        folder: std::path::PathBuf,
        library: Arc<Library>,
        section: Option<notebook::session::Section>,
    }

    impl Fixture {
        fn new(name: &str, titles: &[&str]) -> Self {
            let folder =
                std::env::temp_dir().join(format!("snowbound-undo-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&folder);
            std::fs::create_dir_all(&folder).unwrap();
            let root = folder.join("Notebook");
            let mut notebook =
                Notebook::create(&root, folder.join("cache"), Notebook::NEW_COLOR, &dated())
                    .unwrap();
            notebook.create_section("", "B", &dated()).unwrap();
            let library = Arc::new(Library::created(
                root.to_str().unwrap(),
                notebook,
                &folder.join("cache"),
            ));
            let section = library.open("New Section 1.one", || {}).unwrap();
            let fixture = Self {
                folder,
                library,
                section: Some(section),
            };
            let first = fixture.pages()[0].0;
            fixture.retitle(first, titles[0]);
            for title in &titles[1..] {
                let creation = dated();
                let space = creation.space();
                fixture.edit(vec![Op::Section(SectionOp::Create(creation))]);
                fixture.retitle(space, title);
            }
            fixture
        }

        fn section(&self) -> &notebook::session::Section {
            self.section.as_ref().unwrap()
        }

        fn site(&self) -> Site {
            Site {
                replica: Arc::clone(self.section().replica()),
                library: Arc::clone(&self.library),
                author: AUTHOR.into(),
                fresh: dated(),
            }
        }

        /// Another device's edit, or the user's outside Undo.
        fn edit(&self, ops: Vec<Op>) {
            self.section()
                .replica()
                .apply(
                    AUTHOR,
                    Edit {
                        at: crate::filetime(),
                        ops,
                    },
                )
                .unwrap();
        }

        fn retitle(&self, space: ExGuid, title: &str) {
            let page = self.section().page(space).unwrap();
            self.edit(vec![
                crate::rename::retitled(&page, space, title.into()).unwrap(),
            ]);
        }

        fn pages(&self) -> Vec<(ExGuid, String, u32)> {
            self.section().pages().unwrap()
        }

        fn titles(&self) -> Vec<(String, u32)> {
            self.pages()
                .into_iter()
                .map(|(_, title, level)| (title, level))
                .collect()
        }

        fn space(&self, title: &str) -> ExGuid {
            self.pages()
                .into_iter()
                .find(|(_, listed, _)| listed == title)
                .unwrap()
                .0
        }

        /// The titles in the recycle bin's Deleted Pages.
        fn binned(&self) -> Vec<String> {
            let file = self
                .folder
                .join("Notebook/OneNote_RecycleBin/OneNote_DeletedPages.one");
            std::fs::read(file).map_or_else(
                |_| Vec::new(),
                |bytes| {
                    notebook::session::stored_pages(&bytes)
                        .unwrap()
                        .into_iter()
                        .map(|stored| stored.page.title)
                        .collect()
                },
            )
        }

        /// Does `change`, as the app's thread does, returning what takes it back.
        fn change(&self, change: Change) -> Option<Applied> {
            let shown = self.pages()[0].0;
            self.site().change(change, shown).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            if let Some(section) = self.section.take() {
                let _ = section.close();
            }
            let _ = std::fs::remove_dir_all(&self.folder);
        }
    }

    fn titled(titles: &[&str]) -> Vec<(String, u32)> {
        titles.iter().map(|title| (title.to_string(), 1)).collect()
    }

    /// A page deleted goes to the recycle bin; Undo puts it back where it was and takes it
    /// out of the bin, as OneNote 2010 does; Redo deletes it again. A page another device
    /// put back meanwhile is left alone.
    #[test]
    fn deleted_pages_come_back_where_they_were_and_leave_the_bin() {
        let fixture = Fixture::new("delete", &["First", "Second", "Third"]);
        let second = fixture.space("Second");
        let deleted = fixture
            .change(Change::Delete {
                pages: vec![second],
                show: None,
                created: false,
            })
            .unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Third"]));
        assert_eq!(deleted.show, fixture.space("Third"));
        assert_eq!(fixture.binned(), ["Second"]);
        let restored = fixture.change(deleted.undo).unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Second", "Third"]));
        assert_eq!(restored.show, fixture.space("Second"));
        assert!(fixture.binned().is_empty());
        let again = fixture.change(restored.undo).unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Third"]));
        assert_eq!(fixture.binned(), ["Second"]);

        // Put back elsewhere first, the page stays as that device left it.
        let Change::Restore { pages, .. } = &again.undo else {
            panic!()
        };
        let copy = notebook::session::moved(&pages[0].page, AUTHOR).unwrap();
        fixture.edit(vec![copy]);
        assert!(fixture.change(again.undo).is_none());
        assert_eq!(fixture.titles(), titled(&["First", "Third", "Second"]));
    }

    /// New Page taken back: the page goes for good while it holds nothing, the page shown
    /// before it shows again, and Redo brings it back. Once another device wrote on it,
    /// it stays.
    #[test]
    fn a_new_page_goes_only_while_it_holds_nothing() {
        let fixture = Fixture::new("create", &["First", "Second"]);
        let creation = dated();
        let new = creation.space();
        fixture.edit(vec![Op::Section(SectionOp::Create(creation))]);
        let first = fixture.space("First");
        let removed = fixture
            .change(Change::Delete {
                pages: vec![new],
                show: Some(first),
                created: true,
            })
            .unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Second"]));
        assert_eq!(removed.show, first);
        assert!(fixture.binned().is_empty());
        let back = fixture.change(removed.undo).unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Second", ""]));
        let Change::Delete { pages, .. } = &back.undo else {
            panic!()
        };
        fixture.retitle(pages[0], "Written elsewhere");
        assert!(fixture.change(back.undo).is_none());
        assert_eq!(
            fixture.titles(),
            titled(&["First", "Second", "Written elsewhere"])
        );
    }

    /// Pages moved and indented go back to their places and levels, before the next page
    /// still there when another device deleted the one they followed.
    #[test]
    fn arranged_pages_go_back_before_the_next_page_still_there() {
        let fixture = Fixture::new("arrange", &["A", "B", "C", "D"]);
        let order: Vec<(ExGuid, u32)> = fixture
            .pages()
            .into_iter()
            .map(|(space, _, level)| (space, level))
            .collect();
        let [a, b, c, d] = ["A", "B", "C", "D"].map(|title| fixture.space(title));
        fixture.edit(vec![Op::Section(SectionOp::Pages(vec![
            PageEdit::move_to(a, None, 1).unwrap(),
            PageEdit::set_level(c, 2).unwrap(),
        ]))]);
        assert_eq!(
            fixture.titles(),
            [("B", 1), ("C", 2), ("D", 1), ("A", 1)].map(|(t, l)| (t.to_owned(), l))
        );
        let arrange = Change::Arrange {
            order: order.clone(),
            moved: vec![a, c],
        };
        let arranged = fixture.change(arrange).unwrap();
        assert_eq!(fixture.titles(), titled(&["A", "B", "C", "D"]));
        assert_eq!(arranged.show, a);
        fixture.change(arranged.undo).unwrap();
        fixture.edit(vec![Op::Section(SectionOp::Delete(vec![b]))]);
        fixture
            .change(Change::Arrange {
                order,
                moved: vec![a, c, d],
            })
            .unwrap();
        assert_eq!(fixture.titles(), titled(&["A", "C", "D"]));
    }

    /// A page sent to another section comes back where it was and leaves that section;
    /// gone from there meanwhile, it does not.
    #[test]
    fn a_page_sent_away_comes_back() {
        let fixture = Fixture::new("send", &["First", "Second", "Third"]);
        let target = fixture.library.section_identity("B.one").unwrap();
        let in_b = |fixture: &Fixture| {
            let section = fixture.library.open("B.one", || {}).unwrap();
            let titles: Vec<String> = section
                .pages()
                .unwrap()
                .into_iter()
                .map(|(_, title, _)| title)
                .collect();
            section.close().unwrap();
            titles
        };
        let sent = fixture
            .change(Change::Send {
                page: fixture.space("Second"),
                target,
            })
            .unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Third"]));
        assert_eq!(in_b(&fixture), ["", "Second"]);
        let returned = fixture.change(sent.undo).unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Second", "Third"]));
        assert_eq!(in_b(&fixture), [""]);
        let sent = fixture.change(returned.undo).unwrap();
        let Change::Return { page, .. } = &sent.undo else {
            panic!()
        };
        let section = fixture.library.open("B.one", || {}).unwrap();
        section.delete_pages(&[*page]).unwrap();
        section.close().unwrap();
        assert!(fixture.change(sent.undo).is_none());
        assert_eq!(fixture.titles(), titled(&["First", "Third"]));
    }

    /// A title given in the page list goes back while the page still has it.
    #[test]
    fn a_title_goes_back_while_the_page_has_it() {
        let fixture = Fixture::new("retitle", &["First", "Renamed"]);
        let page = fixture.space("Renamed");
        let back = fixture
            .change(Change::Retitle {
                page,
                from: "Renamed".into(),
                to: "Second".into(),
            })
            .unwrap();
        assert_eq!(fixture.titles(), titled(&["First", "Second"]));
        fixture.retitle(page, "Changed elsewhere");
        assert!(fixture.change(back.undo).is_none());
    }

    /// Carries out `change` as `manage::State::restructure` does.
    fn carry(notebook: &mut Notebook, change: crate::manage::Structure) {
        use crate::manage::Structure;
        match change {
            Structure::Rename { path, name } => drop(notebook.rename(&path, &name).unwrap()),
            Structure::Move { path, folder } => drop(notebook.move_entry(&path, &folder).unwrap()),
            Structure::Reorder { folder, paths } => {
                let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                notebook.reorder(&folder, &paths).unwrap();
            }
            Structure::Return {
                path,
                folder,
                paths,
            } => {
                notebook.move_entry(&path, &folder).unwrap();
                let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                notebook.reorder(&folder, &paths).unwrap();
            }
            _ => unreachable!(),
        }
    }

    /// Sections renamed, reordered and moved into a group go back, the moved one to its
    /// place among the others; renamed again elsewhere, a section keeps that name.
    #[test]
    fn sections_go_back_to_their_names_and_places() {
        use crate::manage::Structure;
        let folder =
            std::env::temp_dir().join(format!("snowbound-undo-sections-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        let root = folder.join("Notebook");
        let mut notebook =
            Notebook::create(&root, folder.join("cache"), Notebook::NEW_COLOR, &dated()).unwrap();
        for name in ["B", "C"] {
            notebook.create_section("", name, &dated()).unwrap();
        }
        notebook.create_group("", "Group").unwrap();
        let order = |notebook: &Notebook, folder: &str| -> Vec<String> {
            let folder = folders(notebook.catalog())
                .into_iter()
                .find(|listed| listed.path == folder)
                .unwrap();
            entries(folder).into_iter().map(|(_, path)| path).collect()
        };
        let opened = Library::created(root.to_str().unwrap(), notebook, &folder.join("cache"));
        let library = |notebook: Notebook| opened.with(notebook);
        let mut notebook = opened.reopen().unwrap();
        let mut steps = Vec::new();
        for change in [
            Structure::Rename {
                path: "B.one".into(),
                name: "Kitchen".into(),
            },
            Structure::Reorder {
                folder: String::new(),
                paths: vec!["C.one".into(), "New Section 1.one".into()],
            },
            Structure::Move {
                path: "New Section 1.one".into(),
                folder: "Group".into(),
            },
        ] {
            let shown = library(notebook);
            steps.push(structure_undo(&shown, &change).unwrap());
            notebook = shown.reopen().unwrap();
            carry(&mut notebook, change);
        }
        assert_eq!(order(&notebook, ""), ["C.one", "Kitchen.one", "Group"]);
        assert_eq!(order(&notebook, "Group"), ["Group/New Section 1.one"]);
        let mut redo = Vec::new();
        while let Some(step) = steps.pop() {
            let shown = library(notebook);
            let (structure, undo) = restructuring(&shown, &step.change).unwrap();
            redo.push(undo);
            notebook = shown.reopen().unwrap();
            carry(&mut notebook, structure);
        }
        assert_eq!(
            order(&notebook, ""),
            ["New Section 1.one", "B.one", "C.one", "Group"]
        );
        // Redo, then Undo of a rename another device changed since leaves it.
        let shown = library(notebook);
        let rename = redo.pop().unwrap();
        let (structure, undo) = restructuring(&shown, &rename.change).unwrap();
        notebook = shown.reopen().unwrap();
        carry(&mut notebook, structure);
        notebook.rename("Kitchen.one", "Pantry").unwrap();
        assert!(restructuring(&library(notebook), &undo.change).is_none());
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// Every kind of step taken back and redone in one notebook, written as the app writes
    /// it. `SNOWBOUND_UNDO_EXPORT` names a new directory that receives it for a cold
    /// reopen in OneNote 2010: "New Section 1" lists First, Second, Fourth and Recreated,
    /// "B" its own page, and Deleted Pages only Third.
    #[test]
    fn every_step_back_is_stored_as_onenote_stores_one() {
        let mut fixture = Fixture::new("export", &["First", "Second", "Third", "Fourth"]);
        let back = |fixture: &Fixture, change| {
            let done = fixture.change(change).unwrap();
            fixture.change(done.undo).unwrap()
        };
        back(
            &fixture,
            Change::Delete {
                pages: vec![fixture.space("Second")],
                show: None,
                created: false,
            },
        );
        fixture.change(Change::Delete {
            pages: vec![fixture.space("Third")],
            show: None,
            created: false,
        });
        let order = fixture
            .pages()
            .into_iter()
            .map(|(space, _, level)| (space, level))
            .collect();
        let fourth = fixture.space("Fourth");
        fixture.edit(vec![Op::Section(SectionOp::Pages(vec![
            PageEdit::move_to(fourth, Some(fixture.space("First")), 1).unwrap(),
        ]))]);
        fixture.change(Change::Arrange {
            order,
            moved: vec![fourth],
        });
        back(
            &fixture,
            Change::Send {
                page: fixture.space("First"),
                target: fixture.library.section_identity("B.one").unwrap(),
            },
        );
        let creation = dated();
        let created = creation.space();
        fixture.edit(vec![Op::Section(SectionOp::Create(creation))]);
        let restored = back(
            &fixture,
            Change::Delete {
                pages: vec![created],
                show: None,
                created: true,
            },
        );
        fixture.retitle(restored.show, "Recreated");
        assert_eq!(
            fixture.titles(),
            titled(&["First", "Second", "Fourth", "Recreated"])
        );
        assert_eq!(fixture.binned(), ["Third"]);

        let mut notebook = fixture.library.reopen().unwrap();
        for change in [
            crate::manage::Structure::Rename {
                path: "B.one".into(),
                name: "Kitchen".into(),
            },
            crate::manage::Structure::Reorder {
                folder: String::new(),
                paths: vec!["B.one".into(), "New Section 1.one".into()],
            },
        ] {
            let shown = fixture.library.with(notebook);
            let undo = structure_undo(&shown, &change).unwrap();
            notebook = shown.reopen().unwrap();
            carry(&mut notebook, change);
            let (structure, _) =
                restructuring(&fixture.library.with(notebook), &undo.change).unwrap();
            notebook = fixture.library.reopen().unwrap();
            carry(&mut notebook, structure);
        }
        let order: Vec<String> = entries(notebook.catalog())
            .into_iter()
            .map(|(_, path)| path)
            .collect();
        assert_eq!(order, ["New Section 1.one", "B.one"]);

        fixture.section.take().unwrap().close().unwrap();
        if let Some(directory) = std::env::var_os("SNOWBOUND_UNDO_EXPORT") {
            copy(
                &fixture.folder.join("Notebook"),
                std::path::Path::new(&directory),
            );
        }
    }

    fn copy(from: &std::path::Path, to: &std::path::Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap().flatten() {
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    /// Steps a page's editor no longer holds, dropped by a change made elsewhere, leave the
    /// timeline; a page gone takes its steps.
    #[test]
    fn steps_follow_the_page_history() {
        let mut timeline = Timeline::default();
        let shown = page(1);
        timeline.edited(shown, [3, 0], false);
        timeline.record(created(page(2)));
        timeline.edited(shown, [1, 0], false);
        assert!(is_action(timeline.next(false, shown, [1, 0], &here())));
        assert!(is_edit(timeline.next(false, shown, [1, 0], &here())));
        timeline.forget(&[shown]);
        assert!(timeline.next(false, shown, [0, 0], &here()).is_none());
    }
}
