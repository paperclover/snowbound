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
    NewSection { folder: String },
    NewGroup { folder: String },
    Rename { path: String, name: String },
    /// Moves a section or group to the recycle bin.
    Delete { path: String },
    /// Moves a section or group into another folder.
    Move { path: String, folder: String },
    /// Orders a folder's sections and groups.
    Reorder { folder: String, paths: Vec<String> },
}

/// Notebook colours for new notebooks, in turn: OneNote 2010's section colours.
const NOTEBOOK_COLORS: [u32; 3] = [0x00e4a88a, 0x0078b0f6, 0x00bba4d5];

/// `current`, the path of a section, after `from` moved to `to`: a section moved itself,
/// or one inside a moved group.
fn follow(current: &str, from: &str, to: &str) -> String {
    match current.strip_prefix(from) {
        Some("") => to.to_owned(),
        Some(rest) if rest.starts_with('/') => format!("{to}{rest}"),
        _ => current.to_owned(),
    }
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

    /// Adds a page at the end of the open section, or a subpage after the open page, and
    /// opens it with its title ready for typing, as OneNote's New Page and New Subpage do.
    pub(crate) fn new_page(&mut self, subpage: bool) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        // A subpage follows the open page and the subpages already under it.
        let (before, level) = if subpage {
            let at = session
                .pages
                .iter()
                .position(|(space, ..)| *space == session.space)
                .ok_or("The open page is not listed")?;
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
        if subpage {
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
        use crate::templates::Choice;
        self.persist()?;
        let session = self.session.as_ref().ok_or("No section is open")?;
        let space = session.space;
        let page = session.section.page(space)?;
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
                let template =
                    canvas::template::find(name).ok_or("That template is not available")?;
                if page.color.is_some() {
                    ops.push(PageOp::Color(None));
                }
                // Art lies under everything else on the page, first in its order.
                let under = page
                    .objects
                    .iter()
                    .find(|object| !matches!(object, PageObject::Image(image) if image.background))
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
            }
            Choice::Color(index) => {
                ops.push(PageOp::Color(Some(canvas::template::PAGE_COLORS[index].1)))
            }
            Choice::More | Choice::Dismiss | Choice::Colors => return Ok(()),
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
        let color = NOTEBOOK_COLORS[self.notebooks.len() % NOTEBOOK_COLORS.len()];
        let (cache, notify) = (self.cache.clone(), notify(self.proxy.clone()));
        self.load(move || {
            let location = std::path::absolute(&root)?.to_string_lossy().into_owned();
            let notebook = Notebook::create(&location, &cache, color, &page)?;
            let library = Arc::new(Library::open_notebook(&location, notebook, &cache));
            let path = library
                .first_section()
                .ok_or("The new notebook has no section")?;
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok((Loaded::Section(Box::new(session)), page))
        });
        Ok(())
    }

    /// Closes `library`: its files stay, and the sidebar and the next launch leave it out.
    pub(crate) fn close_notebook(&mut self, library: &Arc<Library>) {
        self.notebooks.retain(|open| !Arc::ptr_eq(open, library));
        if self
            .session
            .as_ref()
            .is_some_and(|session| Arc::ptr_eq(&session.library, library))
        {
            self.persist().unwrap_or_else(|error| eprintln!("{error}"));
            self.session = None;
            self.templates = crate::templates::View::Strip;
            match self
                .notebooks
                .iter()
                .find_map(|open| Some((Arc::clone(open), open.first_section()?)))
            {
                Some((open, path)) => self.commands.push(Command::OpenSection(open, path)),
                None => self.title(),
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
                Err(error) => return platform::alert("Couldn't add the section", &error.to_string()),
            },
            _ => None,
        };
        let current = self
            .session
            .as_ref()
            .filter(|session| Arc::ptr_eq(&session.library, &library))
            .map(|session| session.tabs[session.tab].path.clone());
        let (cache, notify) = (self.cache.clone(), notify(self.proxy.clone()));
        self.load(move || {
            let mut notebook = Notebook::open(&library.location, &cache)?;
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
            let open = match change {
                Structure::NewSection { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section", true);
                    let page = page.ok_or("The new section has no page")?;
                    Some(notebook.create_section(&folder, &name, &page)?)
                }
                Structure::NewGroup { folder } => {
                    let name = unused(&names(&notebook, &folder), "New Section Group", false);
                    notebook.create_group(&folder, &name)?;
                    current
                }
                Structure::Rename { path, name } => {
                    let renamed = notebook.rename(&path, &name)?;
                    current.map(|current| follow(&current, &path, &renamed))
                }
                Structure::Delete { path } => {
                    notebook.delete(&path)?;
                    current
                }
                Structure::Move { path, folder } => {
                    let moved = notebook.move_entry(&path, &folder)?;
                    current.map(|current| follow(&current, &path, &moved))
                }
                Structure::Reorder { folder, paths } => {
                    let paths: Vec<&str> = paths.iter().map(String::as_str).collect();
                    notebook.reorder(&folder, &paths)?;
                    current
                }
            };
            let library = Arc::new(Library::open_notebook(&library.location, notebook, &cache));
            let path = open
                .filter(|path| library.contains(path))
                .or_else(|| library.first_section())
                .ok_or("The notebook has no sections left")?;
            let section = library.open(&path, notify)?;
            let (session, page) = read_session(section, library, path, None)?;
            Ok((Loaded::Section(Box::new(session)), page))
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
        let notebook = match &session.library.notebook {
            Ok(Some(_)) => Some((session.library.location.clone(), self.cache.clone())),
            _ => None,
        };
        let replica = Arc::clone(session.section.replica());
        let author = self.author.clone();
        self.load(move || {
            if let Some((location, cache)) = notebook {
                Notebook::open(&location, &cache)?.recycle_pages(&pages, &author)?;
            }
            replica.apply(
                &author,
                Edit {
                    at: crate::filetime(),
                    ops,
                },
            )?;
            Ok((Loaded::Page(next), replica.page(next)?))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moved_sections_are_followed_and_new_names_take_the_next_number() {
        assert_eq!(follow("Group/Inner.one", "Group", "Kept"), "Kept/Inner.one");
        assert_eq!(follow("Group.one", "Group", "Kept"), "Group.one");
        assert_eq!(follow("A.one", "A.one", "B/A.one"), "B/A.one");
        let taken = ["New Section 1".to_owned(), "new section 2".to_owned()];
        assert_eq!(unused(&taken, "New Section", true), "New Section 3");
        assert_eq!(unused(&[], "New Section Group", false), "New Section Group");
        assert_eq!(
            unused(&["New Section Group".to_owned()], "New Section Group", false),
            "New Section Group 2"
        );
    }
}
