//! Changes to notebooks, sections, section groups and pages, as OneNote 2010 makes them:
//! structure through `notebook::session::Notebook`, pages as ops on the open section.

use crate::{Command, Library, Loaded, State, notify, platform, read_session};
use notebook::session::Notebook;
use onestore::{
    ExGuid, PageCreation, PageEdit,
    op::{Edit, Op, PageOp, SectionOp},
    page::{Image, Page, PageObject},
};
use std::{error::Error, sync::Arc};

/// A change to a notebook's sections and groups, by catalog path.
pub enum Structure {
    NewSection {
        folder: String,
    },
    NewGroup {
        folder: String,
    },
    Rename {
        path: String,
        name: String,
    },
    /// Moves a section or group to the recycle bin.
    Delete {
        path: String,
    },
    /// Moves a section or group into another folder.
    Move {
        path: String,
        folder: String,
    },
    /// Orders a folder's sections and groups.
    Reorder {
        folder: String,
        paths: Vec<String>,
    },
}

/// The colour OneNote 2010 gave each new notebook beside its default one, COLORREF
/// (`corpus/notebook-management/native/new-notebook`).
const NOTEBOOK_COLOR: u32 = 0x00aeba91;

/// `current`, the path of a section, after `from` moved to `to`: a section moved itself,
/// or one inside a moved group.
fn follow(current: &str, from: &str, to: &str) -> String {
    match current.strip_prefix(from) {
        Some("") => to.to_owned(),
        Some(rest) if rest.starts_with('/') => format!("{to}{rest}"),
        _ => current.to_owned(),
    }
}

/// The section `library` shows after a change: `open` where it still lists it, else its
/// first, or none once it has no sections.
fn shown(library: &Library, open: Option<String>) -> Option<String> {
    open.filter(|path| library.contains(path))
        .or_else(|| library.first_section())
}

/// The first of `base`, `base 2`, `base 3`… that `taken` does not hold, as OneNote names
/// new sections ("New Section 1", …) and groups.
fn unused(taken: &[String], base: &str, numbered: bool) -> String {
    (1..)
        .map(|n| match (numbered, n) {
            (false, 1) => base.to_owned(),
            _ => format!("{base} {n}"),
        })
        .find(|name| !taken.iter().any(|taken| taken.eq_ignore_ascii_case(name)))
        .expect("Some number is free")
}

impl State {
    /// A page OneNote 2010 would create now: titled, with its date and time.
    fn dated_page(&self, before: Option<ExGuid>) -> Result<PageCreation, Box<dyn Error>> {
        let creation = PageCreation::new(before, Some(""), &self.author)?;
        let [date, time] = platform::date_text(creation.created());
        Ok(creation.dated(&date, &time)?)
    }

    /// Adds a page at the end of the open section, or a subpage of page `under`, and opens
    /// it with its title ready for typing, as OneNote's New Page and New Subpage do.
    pub(crate) fn new_page(&mut self, under: Option<ExGuid>) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        // A subpage follows its page and the subpages already under it.
        let (before, level) = if let Some(under) = under {
            let at = session
                .pages
                .iter()
                .position(|(space, ..)| *space == under)
                .ok_or("That page is not listed")?;
            let level = session.pages[at].2;
            let next = session.pages[at + 1..]
                .iter()
                .find(|(.., below)| *below <= level)
                .map(|(space, ..)| *space);
            (next, (level + 1).min(3))
        } else {
            (None, 1)
        };
        let creation = self.dated_page(None)?;
        let space = creation.space();
        let mut ops = vec![Op::Section(SectionOp::Create(creation))];
        if under.is_some() {
            ops.push(Op::Section(SectionOp::Pages(vec![PageEdit::move_to(
                space, before, level,
            )?])));
        }
        let session = self.session.as_mut().ok_or("No section is open")?;
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops,
            },
        )?;
        session.status = "Saving";
        self.title_focus = Some(space);
        self.commands.push(Command::OpenPage(space));
        Ok(())
    }

    /// Gives the open page `choice`'s background: a template's art in place of any it
    /// had, or a page colour instead of art.
    pub(crate) fn apply_template(
        &mut self,
        choice: crate::templates::Choice,
    ) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        let space = session.space;
        let ops = template_ops(&session.section.page(space)?, choice)?;
        if ops.is_empty() {
            return Ok(());
        }
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: ops.into_iter().map(|op| Op::Page { space, op }).collect(),
            },
        )?;
        self.refresh()
    }

    /// Asks where to create a notebook and creates it, as File, New does in OneNote.
    pub(crate) fn new_notebook(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(root) = platform::pick_new("New Notebook", "My Notebook") else {
            return Ok(());
        };
        let page = self.dated_page(None)?;
        let (cache, notify) = (self.cache.clone(), notify(self.proxy.clone()));
        self.load(move || {
            let location = std::path::absolute(&root)?.to_string_lossy().into_owned();
            let notebook = Notebook::create(&location, &cache, NOTEBOOK_COLOR, &page)?;
            let library = Arc::new(Library::created(&location, notebook, &cache));
            let path = library
                .first_section()
                .ok_or("The new notebook has no section")?;
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok(Loaded::Section(Box::new(session), page))
        });
        Ok(())
    }

    /// Closes `library`: its files stay, and the sidebar and the next launch leave it out.
    pub(crate) fn close_notebook(&mut self, library: &Arc<Library>) {
        self.notebooks.retain(|open| !Arc::ptr_eq(open, library));
        let shown = |shown: &Arc<Library>| shown.location == library.location;
        if self.sectionless.as_ref().is_some_and(shown)
            || self
                .session
                .as_ref()
                .is_some_and(|session| shown(&session.library))
        {
            self.persist().unwrap_or_else(|error| eprintln!("{error}"));
            self.session = None;
            self.sectionless = None;
            self.templates = crate::templates::View::Strip;
            match self
                .notebooks
                .iter()
                .find_map(|open| Some((Arc::clone(open), open.first_section()?)))
            {
                Some((open, path)) => self.commands.push(Command::OpenSection(open, path)),
                None => {
                    self.sectionless = self.notebooks.first().cloned();
                    self.title();
                }
            }
        }
        self.save_settings();
    }

    /// Changes `library`'s sections and groups on a thread of its own, then shows the
    /// section the change leaves open: a new or moved one, or the one shown before.
    pub(crate) fn restructure(&mut self, library: Arc<Library>, change: Structure) {
        let page = match change {
            Structure::NewSection { .. } => match self.dated_page(None) {
                Ok(page) => Some(page),
                Err(error) => {
                    return platform::alert("Couldn't add the section", &error.to_string());
                }
            },
            _ => None,
        };
        let current = self
            .session
            .as_ref()
            .filter(|session| Arc::ptr_eq(&session.library, &library))
            .map(|session| session.tabs[session.tab].path.clone());
        let notify = notify(self.proxy.clone());
        self.load(move || {
            let mut notebook = library.reopen()?;
            let names = |notebook: &Notebook, folder: &str| -> Vec<String> {
                let mut folders = vec![notebook.catalog()];
                while let Some(candidate) = folders.pop() {
                    if candidate.path == folder {
                        return candidate
                            .sections
                            .iter()
                            .map(|section| section.path.clone())
                            .chain(candidate.groups.iter().map(|group| group.path.clone()))
                            .map(|path| {
                                let name = path.rsplit('/').next().unwrap_or_default();
                                name.strip_suffix(".one").unwrap_or(name).to_owned()
                            })
                            .collect();
                    }
                    folders.extend(&candidate.groups);
                }
                Vec::new()
            };
            // The open section where the change leaves it, and the section to show.
            let (followed, created) = match change {
                Structure::NewSection { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section", true);
                    let page = page.ok_or("The new section has no page")?;
                    (
                        current,
                        Some(notebook.create_section(&folder, &name, &page)?),
                    )
                }
                Structure::NewGroup { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section Group", false);
                    notebook.create_group(&folder, &name)?;
                    (current, None)
                }
                Structure::Rename { path, name } => {
                    let renamed = notebook.rename(&path, &name)?;
                    (
                        current.map(|current| follow(&current, &path, &renamed)),
                        None,
                    )
                }
                Structure::Delete { path } => {
                    notebook.delete(&path)?;
                    (current, None)
                }
                Structure::Move { path, folder } => {
                    let moved = notebook.move_entry(&path, &folder)?;
                    (current.map(|current| follow(&current, &path, &moved)), None)
                }
                Structure::Reorder { folder, paths } => {
                    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                    notebook.reorder(&folder, &paths)?;
                    (current, None)
                }
            };
            let fresh = created.is_some();
            let open = created.or(followed.clone());
            let library = Arc::new(library.with(notebook));
            let Some(path) = shown(&library, open) else {
                return Ok(Loaded::Library(library, None));
            };
            // The open section, perhaps renamed or moved, stays open: its replica is this
            // thread's to reopen only where no section holds it.
            if followed.as_deref() == Some(path.as_str()) {
                return Ok(Loaded::Library(library, Some(path)));
            }
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok(if fresh {
                Loaded::Created(Box::new(session), page)
            } else {
                Loaded::Section(Box::new(session), page)
            })
        });
    }

    /// Deletes pages of the open section as OneNote 2010 does: each goes to the notebook's
    /// recycle bin, then leaves the section. A section left without pages gains a new one.
    pub(crate) fn delete_pages(&mut self, spaces: Vec<ExGuid>) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        let pages: Vec<Page> = spaces
            .iter()
            .map(|space| session.section.page(*space))
            .collect::<Result<_, _>>()?;
        let remaining: Vec<ExGuid> = session
            .pages
            .iter()
            .map(|(space, ..)| *space)
            .filter(|space| !spaces.contains(space))
            .collect();
        // OneNote shows the page after the first deleted one, or the one before it.
        let at = session
            .pages
            .iter()
            .position(|(space, ..)| spaces.contains(space))
            .unwrap_or_default();
        let next = session.pages[at..]
            .iter()
            .chain(session.pages[..at].iter().rev())
            .map(|(space, ..)| *space)
            .find(|space| remaining.contains(space));
        let mut ops = vec![Op::Section(SectionOp::Delete(spaces))];
        let next = match next {
            Some(next) => next,
            None => {
                let creation = self.dated_page(None)?;
                let space = creation.space();
                ops.push(Op::Section(SectionOp::Create(creation)));
                space
            }
        };
        let notebook =
            matches!(session.library.notebook, Ok(Some(_))).then(|| Arc::clone(&session.library));
        let replica = Arc::clone(session.section.replica());
        let author = self.author.clone();
        self.load(move || {
            if let Some(library) = notebook {
                library.reopen()?.recycle_pages(&pages, &author)?;
            }
            replica.apply(
                &author,
                Edit {
                    at: crate::filetime(),
                    ops,
                },
            )?;
            Ok(Loaded::Page(next, replica.page(next)?))
        });
        Ok(())
    }

    /// Moves page `space` of the open section to the end of the section at catalog `path`,
    /// as OneNote moves a page dropped on a section's tab: the page keeps its identity,
    /// title, date and content there and leaves this section.
    pub(crate) fn move_page(&mut self, space: ExGuid, path: String) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        let import = notebook::session::moved(&session.section.page(space)?, &self.author)?;
        let at = session
            .pages
            .iter()
            .position(|(listed, ..)| *listed == space)
            .unwrap_or_default();
        let next = session.pages[at + 1..]
            .iter()
            .chain(session.pages[..at].iter().rev())
            .map(|(listed, ..)| *listed)
            .next();
        let mut ops = vec![Op::Section(SectionOp::Delete(vec![space]))];
        let next = match next {
            Some(next) => next,
            None => {
                let fresh = self.dated_page(None)?;
                let space = fresh.space();
                ops.push(Op::Section(SectionOp::Create(fresh)));
                space
            }
        };
        let library = Arc::clone(&session.library);
        let replica = Arc::clone(session.section.replica());
        let author = self.author.clone();
        self.load(move || {
            let target = library.open(&path, || {})?;
            target.replica().apply(
                &author,
                Edit {
                    at: crate::filetime(),
                    ops: vec![import],
                },
            )?;
            target.close()?;
            replica.apply(
                &author,
                Edit {
                    at: crate::filetime(),
                    ops,
                },
            )?;
            Ok(Loaded::Page(next, replica.page(next)?))
        });
        Ok(())
    }

    /// Moves or indents pages of the open section, in order.
    pub(crate) fn edit_pages(&mut self, edits: Vec<PageEdit>) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_mut().ok_or("No section is open")?;
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: vec![Op::Section(SectionOp::Pages(edits))],
            },
        )?;
        session.status = "Saving";
        session.pages = session.section.pages()?;
        Ok(())
    }
}

/// The ops giving `page` `choice`'s background: its template art (OneNote's pictures'
/// places, our recreations' bytes) or a page colour, in place of the background it had.
pub fn template_ops(
    page: &Page,
    choice: crate::templates::Choice,
) -> Result<Vec<PageOp>, Box<dyn Error>> {
    use crate::templates::Choice;
    let mut ops: Vec<PageOp> = page
        .objects
        .iter()
        .filter(|object| matches!(object, PageObject::Image(image) if image.background))
        .map(|object| PageOp::Delete {
            object: object.id(),
        })
        .collect();
    match choice {
        Choice::Template(name) => {
            let template = canvas::template::find(name).ok_or("That template is not available")?;
            if page.color.is_some() {
                ops.push(PageOp::Color(None));
            }
            // Art lies under everything else on the page, first of its children (the title
            // is no child).
            let under = page
                .objects
                .iter()
                .find(|object| match object {
                    PageObject::Title(_) => false,
                    PageObject::Image(image) => !image.background,
                    _ => true,
                })
                .map(PageObject::id);
            for art in template.art {
                let bytes = canvas::gpu::page::template_picture(art.art)
                    .ok_or("That template's art is missing")?;
                ops.push(PageOp::Add {
                    object: PageObject::Image(Image {
                        id: onestore::page::text::new_id()?,
                        layout: onestore::document::Layout {
                            x: Some(art.position[0]),
                            y: Some(art.position[1]),
                            max_width: art.size.map(|size| size[0]),
                            max_height: art.size.map(|size| size[1]),
                            width_set_by_user: art.size.map(|_| true),
                            ..Default::default()
                        },
                        size: art.size,
                        bytes: Some(bytes.into()),
                        alt: None,
                        background: true,
                    }),
                    before: under,
                });
            }
            // The one template with content worth keeping.
            if name == "Informal Meeting Notes" {
                ops.extend(onestore::op::lower_page(
                    page,
                    &crate::meeting::content(page)?,
                )?);
            }
        }
        Choice::Color(index) => {
            ops.push(PageOp::Color(Some(canvas::template::PAGE_COLORS[index].1)))
        }
        Choice::More | Choice::Dismiss | Choice::Colors => return Ok(Vec::new()),
    }
    Ok(ops)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::Choice;
    use std::path::Path;

    const AUTHOR: &str = "Rust Author";

    fn dated() -> PageCreation {
        let creation = PageCreation::new(None, Some(""), AUTHOR).unwrap();
        let [date, time] = platform::date_text(creation.created());
        creation.dated(&date, &time).unwrap()
    }

    /// Applies `ops` to the section file at `file` as one edit, as the section thread does,
    /// and publishes it.
    fn edit(file: &Path, ops: Vec<Op>) {
        let arena = onestore::Arena::default();
        let mut section =
            onestore::Section::open(&arena, onestore::read_file(file).unwrap()).unwrap();
        section
            .apply(
                AUTHOR,
                &Edit {
                    at: crate::filetime(),
                    ops,
                },
            )
            .unwrap();
        if let Some(transaction) = section.seal().unwrap() {
            transaction.commit_file(file).unwrap();
        }
    }

    fn pages(file: &Path) -> Vec<(ExGuid, String, u32)> {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, onestore::read_file(file).unwrap())
            .unwrap()
            .pages()
            .unwrap()
    }

    fn page(file: &Path, space: ExGuid) -> Page {
        let arena = onestore::Arena::default();
        onestore::Section::open(&arena, onestore::read_file(file).unwrap())
            .unwrap()
            .page(space)
            .unwrap()
    }

    fn copy(from: &Path, to: &Path) {
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

    /// A notebook made and changed the way the app makes and changes one: created with a
    /// dated first page; sections and groups added, renamed, moved, reordered and deleted to
    /// the recycle bin; pages titled, given a template's art or a colour, made subpages and
    /// deleted to the recycle bin. `SNOWBOUND_MANAGEMENT_EXPORT` names a new directory that
    /// receives it for a cold reopen in OneNote 2010.
    #[test]
    fn a_notebook_is_managed_as_onenote_manages_one() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-manage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("Managed");
        let cache = temporary.join("cache");
        let mut notebook = Notebook::create(&root, &cache, NOTEBOOK_COLOR, &dated()).unwrap();
        for (folder, name) in [("", "New Section 2"), ("", "Binned")] {
            notebook.create_section(folder, name, &dated()).unwrap();
        }
        for (group, section) in [("Kept", "Inner"), ("Doomed", "Gone")] {
            notebook.create_group("", group).unwrap();
            notebook.create_section(group, section, &dated()).unwrap();
        }

        // Pages of the first section, each titled.
        let file = root.join("New Section 1.one");
        let first = pages(&file)[0].0;
        let title = |space: ExGuid, text: &str| {
            let page = page(&file, space);
            let title = page
                .objects
                .iter()
                .find_map(|object| match object {
                    PageObject::Title(title) => title.outlines[0].paragraphs[0].text(),
                    _ => None,
                })
                .unwrap()
                .id;
            Op::Page {
                space,
                op: PageOp::Text {
                    text: title,
                    range: 0..0,
                    with: text.to_owned(),
                },
            }
        };
        edit(&file, vec![title(first, "Garden plan")]);
        let mut spaces = Vec::new();
        for text in [
            "Ivy page",
            "Teal page",
            "Subpage",
            "Deleted page",
            "Meeting page",
        ] {
            let creation = dated();
            let space = creation.space();
            edit(&file, vec![Op::Section(SectionOp::Create(creation))]);
            edit(&file, vec![title(space, text)]);
            spaces.push(space);
        }
        let [ivy, teal, subpage, deleted, meeting] = spaces[..] else {
            unreachable!()
        };
        let background = |space: ExGuid, choice: Choice| {
            template_ops(&page(&file, space), choice)
                .unwrap()
                .into_iter()
                .map(|op| Op::Page { space, op })
                .collect::<Vec<_>>()
        };
        edit(&file, background(ivy, Choice::Template("Ivy")));
        edit(
            &file,
            background(meeting, Choice::Template("Informal Meeting Notes")),
        );
        let teal_index = canvas::template::PAGE_COLORS
            .iter()
            .position(|(name, _)| *name == "Teal")
            .unwrap();
        edit(&file, background(teal, Choice::Color(teal_index)));
        edit(
            &file,
            vec![Op::Section(SectionOp::Pages(vec![
                PageEdit::set_level(subpage, 2).unwrap(),
            ]))],
        );
        let recycled = page(&file, deleted);
        notebook
            .recycle_pages(std::slice::from_ref(&recycled), AUTHOR)
            .unwrap();
        edit(&file, vec![Op::Section(SectionOp::Delete(vec![deleted]))]);

        // A page moved to another section, as a drop on its tab moves it.
        let moved_space = {
            let creation = dated();
            let space = creation.space();
            edit(&file, vec![Op::Section(SectionOp::Create(creation))]);
            edit(&file, vec![title(space, "Moved page")]);
            space
        };
        let moving = page(&file, moved_space);
        let other = root.join("New Section 2.one");
        edit(
            &other,
            vec![notebook::session::moved(&moving, AUTHOR).unwrap()],
        );
        edit(
            &file,
            vec![Op::Section(SectionOp::Delete(vec![moved_space]))],
        );
        let arrived = pages(&other).last().unwrap().0;
        let arrived = page(&other, arrived);
        assert_eq!(
            (
                arrived.title.as_str(),
                arrived.identity,
                arrived.date_text()
            ),
            ("Moved page", moving.identity, moving.date_text())
        );

        // Structure.
        assert_eq!(
            notebook.rename("New Section 2.one", "Kitchen").unwrap(),
            "Kitchen.one"
        );
        assert_eq!(
            notebook.move_entry("Kitchen.one", "Kept").unwrap(),
            "Kept/Kitchen.one"
        );
        assert_eq!(notebook.rename("Kept", "Archive").unwrap(), "Archive");
        notebook.delete("Doomed").unwrap();
        notebook.delete("Binned.one").unwrap();
        notebook
            .reorder("", &["Archive", "New Section 1.one"])
            .unwrap();

        let catalog = notebook.catalog();
        let names = |folder: &notebook::discover::Folder| {
            folder
                .sections
                .iter()
                .map(|section| section.path.clone())
                .chain(folder.groups.iter().map(|group| group.path.clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(catalog),
            ["New Section 1.one", "Archive", "OneNote_RecycleBin"]
        );
        assert_eq!(
            names(&catalog.groups[0]),
            ["Archive/Inner.one", "Archive/Kitchen.one"]
        );
        let mut binned = names(&catalog.groups[1]);
        binned.sort();
        assert_eq!(
            binned,
            [
                "OneNote_RecycleBin/Binned.one",
                "OneNote_RecycleBin/Gone.one",
                "OneNote_RecycleBin/OneNote_DeletedPages.one",
            ]
        );
        assert!(!root.join("Doomed").exists());

        let listed = pages(&file);
        assert_eq!(
            listed
                .iter()
                .map(|(_, title, level)| (title.as_str(), *level))
                .collect::<Vec<_>>(),
            [
                ("Garden plan", 1),
                ("Ivy page", 1),
                ("Teal page", 1),
                ("Subpage", 2),
                ("Meeting page", 1)
            ]
        );
        let shown = page(&file, teal);
        assert_eq!(
            shown.color,
            Some(canvas::template::PAGE_COLORS[teal_index].1)
        );
        assert!(shown.date_text().is_some());
        assert!(
            page(&file, ivy)
                .objects
                .iter()
                .any(|object| matches!(object, PageObject::Image(image) if image.background))
        );
        let written = page(&file, meeting);
        let outlines: Vec<_> = written
            .objects
            .iter()
            .filter_map(|object| match object {
                PageObject::Outline(outline) => Some(outline),
                _ => None,
            })
            .collect();
        assert_eq!(outlines.len(), 4);
        let lines: Vec<&str> = outlines
            .iter()
            .flat_map(|outline| &outline.paragraphs)
            .filter_map(|paragraph| paragraph.text())
            .map(|text| text.text.text())
            .collect();
        assert!(lines.contains(&"Meeting Details") && lines.contains(&"Attendees:"));
        assert!(
            outlines
                .iter()
                .flat_map(|outline| &outline.paragraphs)
                .filter_map(|paragraph| paragraph.text())
                .any(|text| !text.tags.is_empty())
        );
        let bin = root.join("OneNote_RecycleBin/OneNote_DeletedPages.one");
        let [(space, title, 1)] = &pages(&bin)[..] else {
            panic!("{:?}", pages(&bin))
        };
        assert_eq!(title, "Deleted page");
        let kept = page(&bin, *space);
        assert_eq!(
            (kept.identity, kept.created),
            (recycled.identity, recycled.created)
        );
        assert_eq!(kept.date_text(), recycled.date_text());

        if let Some(directory) = std::env::var_os("SNOWBOUND_MANAGEMENT_EXPORT") {
            copy(&root, Path::new(&directory));
        }
        std::fs::remove_dir_all(&temporary).unwrap();
    }

    /// Deleting a group's last section shows the notebook's other one; deleting that too
    /// leaves the notebook showing no section, where it failed before.
    #[test]
    fn deleting_every_section_leaves_the_notebook_showing_none() {
        let temporary =
            std::env::temp_dir().join(format!("snowbound-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temporary);
        std::fs::create_dir_all(&temporary).unwrap();
        let root = temporary.join("Emptied");
        let location = root.to_str().unwrap();
        let cache = temporary.join("cache");
        let mut notebook = Notebook::create(location, &cache, NOTEBOOK_COLOR, &dated()).unwrap();
        notebook.create_group("", "Group").unwrap();
        notebook.create_section("Group", "Inner", &dated()).unwrap();

        notebook.delete("Group/Inner.one").unwrap();
        let library = Library::created(location, notebook, &cache);
        assert_eq!(
            shown(&library, Some("Group/Inner.one".into())).as_deref(),
            Some("New Section 1.one")
        );

        let mut notebook = library.reopen().unwrap();
        notebook.delete("New Section 1.one").unwrap();
        let library = library.with(notebook);
        assert_eq!(shown(&library, Some("New Section 1.one".into())), None);
        assert!(library.tabs("").is_empty());
        std::fs::remove_dir_all(&temporary).unwrap();
    }

    #[test]
    fn moved_sections_are_followed_and_new_names_take_the_next_number() {
        assert_eq!(follow("Group/Inner.one", "Group", "Kept"), "Kept/Inner.one");
        assert_eq!(follow("Group.one", "Group", "Kept"), "Group.one");
        assert_eq!(follow("A.one", "A.one", "B/A.one"), "B/A.one");
        let taken = ["New Section 1".to_owned(), "new section 2".to_owned()];
        assert_eq!(unused(&taken, "New Section", true), "New Section 3");
        assert_eq!(unused(&[], "New Section Group", false), "New Section Group");
        assert_eq!(
            unused(
                &["New Section Group".to_owned()],
                "New Section Group",
                false
            ),
            "New Section Group 2"
        );
    }
}
