//! Renaming in place, as VS Code renames a file and OneNote 2010 a section's tab: a field
//! over the name with it all selected, where Enter or a press elsewhere keeps the name typed
//! and Escape the old one.

use crate::{Command, Library, State, manage::Structure};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    page::{Page, PageObject},
};
use std::{error::Error, sync::Arc};
use ui::{Event, Id, Spec, Theme, Ui, fill, px};
use winit::keyboard::{Key, NamedKey};

/// Room between the field's edge and its text.
pub const PAD: f32 = 3.0;

/// What is being renamed, and the name typed so far.
pub struct Renaming {
    pub target: Target,
    pub name: String,
}

pub enum Target {
    /// A section or group by catalog path, in its sidebar row or, `in_tab`, its tab.
    Entry {
        library: Arc<Library>,
        path: String,
        in_tab: bool,
    },
    /// A page of the open section, by its title in its tab.
    Page(ExGuid),
    /// A notebook, in its sidebar row, as Notebook Properties renames it.
    Notebook(Arc<Library>),
}

impl Renaming {
    /// Whether the sidebar row or tab of the section or group at `path` shows the field.
    pub fn entry(&self, library: &Arc<Library>, path: &str, tab: bool) -> bool {
        matches!(&self.target, Target::Entry { library: renamed, path: at, in_tab }
            if Arc::ptr_eq(renamed, library) && at == path && *in_tab == tab)
    }

    pub fn notebook(&self, library: &Arc<Library>) -> bool {
        matches!(&self.target, Target::Notebook(renamed) if Arc::ptr_eq(renamed, library))
    }

    pub fn page(&self, space: ExGuid) -> bool {
        matches!(self.target, Target::Page(renamed) if renamed == space)
    }
}

pub fn field() -> Id {
    Id::ROOT.child("rename")
}

/// The rename field over the label of a section tab `tall`, editing `name`, as `edit`.
pub fn tab_field(ui: &mut Ui, theme: &Theme, name: &mut String, tall: f32) -> Option<bool> {
    let width = ui.measure(name)[0] + 4.0 + 2.0 * PAD;
    ui.open(
        "rename",
        Spec {
            flags: ui::Flags::FLOAT,
            size: [px(width.max(48.0)), px(tall - 6.0)],
            position: [ui::shell::TAB_PAD - PAD, 3.0],
            ..Spec::default()
        },
    );
    let kept = edit(ui, theme, name, tall - 6.0);
    ui.close();
    kept
}

/// The rename field, `height` tall, editing `name`: whether Enter kept or Escape dropped
/// the name typed.
pub fn edit(ui: &mut Ui, theme: &Theme, name: &mut String, height: f32) -> Option<bool> {
    let signal = ui::text_field(
        ui,
        field(),
        name,
        "",
        Spec {
            size: [fill(), px(height)],
            fill: Some(theme.base),
            border: Some(theme.accent),
            radius: 2.0,
            pad: [PAD, 0.0],
            ..Spec::default()
        },
    );
    crate::name(ui, field(), "Name");
    signal.events.iter().find_map(|event| match event {
        Event::Key {
            key: Key::Named(key @ (NamedKey::Enter | NamedKey::Escape)),
            ..
        } => Some(*key == NamedKey::Enter),
        _ => None,
    })
}

impl State {
    /// Opens the rename field on `target` with its name selected.
    pub(crate) fn rename(&mut self, target: Target) {
        let name = match &target {
            Target::Entry { path, in_tab, .. } => {
                self.sidebar |= !in_tab;
                crate::library::entry_name(path).to_owned()
            }
            Target::Page(space) => self
                .session
                .as_ref()
                .and_then(|session| session.pages.iter().find(|(page, ..)| page == space))
                .map(|(_, title, _)| title.clone())
                .unwrap_or_default(),
            Target::Notebook(library) => {
                self.sidebar = true;
                library.name.clone()
            }
        };
        self.renaming = Some(Renaming { target, name });
        self.ui.focus_all(field());
    }

    /// Closes the rename field, renaming what it was on to the name typed where `keep`.
    pub(crate) fn finish_renaming(&mut self, keep: bool) {
        let Some(Renaming { target, name }) = self.renaming.take() else {
            return;
        };
        self.ui.set_focus(Some(crate::page()));
        let name = name.trim().to_owned();
        if !keep || name.is_empty() {
            return;
        }
        match target {
            Target::Entry { library, path, .. } => {
                if name != crate::library::entry_name(&path) {
                    self.commands.push(Command::Structure(
                        library,
                        Structure::Rename { path, name },
                    ));
                }
            }
            // As OK in Notebook Properties: the folder takes the name where it can.
            Target::Notebook(library) if name != library.name => {
                if library.renamed_location(&name).is_some() {
                    self.rename_notebook(library, name, None);
                } else {
                    self.commands.push(Command::Structure(
                        library,
                        Structure::Properties { name, color: None },
                    ));
                }
            }
            Target::Notebook(_) => {}
            Target::Page(space) => {
                if let Err(error) = self.retitle(space, name) {
                    crate::platform::alert(
                        "Couldn't rename the page",
                        &crate::plain(&*error, "page"),
                    );
                }
            }
        }
    }

    /// Gives page `space` of the open section the title `name`.
    fn retitle(&mut self, space: ExGuid, name: String) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_mut().ok_or("No section is open")?;
        let op = retitled(&session.section.page(space)?, space, name.clone())?;
        let old = session
            .pages
            .iter()
            .find(|(page, ..)| *page == space)
            .map(|(_, title, _)| title.clone())
            .unwrap_or_default();
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: vec![op],
            },
        )?;
        session.pages = session.section.pages()?;
        self.made(crate::undo::Change::Retitle {
            page: space,
            from: name,
            to: old,
        });
        self.edited(vec![space]);
        self.refresh()
    }
}

/// The op titling `page`, in `space`, `name`.
pub fn retitled(page: &Page, space: ExGuid, name: String) -> Result<Op, Box<dyn Error>> {
    let title = page
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Title(title) => title.outlines.first()?.paragraphs.first()?.text(),
            _ => None,
        })
        .ok_or("The page has no title to rename")?;
    let length = title.text.utf16_offset(title.text.text().len())?;
    Ok(Op::Page {
        space,
        op: PageOp::Text {
            text: title.id,
            range: 0..length,
            with: name,
        },
    })
}
