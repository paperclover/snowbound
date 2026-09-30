//! Renaming in place, as VS Code renames a file and OneNote 2010 a section's tab: a field
//! over the name with it all selected, where Enter or a press elsewhere keeps the name typed
//! and Escape the old one.

use crate::{Command, Library, State, manage::Structure};
use onestore::{
    ExGuid,
    op::{Edit, Op, PageOp},
    page::PageObject,
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
}

impl Renaming {
    /// Whether the sidebar row or tab of the section or group at `path` shows the field.
    pub fn entry(&self, library: &Arc<Library>, path: &str, tab: bool) -> bool {
        matches!(&self.target, Target::Entry { library: renamed, path: at, in_tab }
            if Arc::ptr_eq(renamed, library) && at == path && *in_tab == tab)
    }

    pub fn page(&self, space: ExGuid) -> bool {
        matches!(self.target, Target::Page(renamed) if renamed == space)
    }
}

pub fn field() -> Id {
    Id::ROOT.child("rename")
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
                let name = path.rsplit('/').next().unwrap_or_default();
                name.strip_suffix(".one").unwrap_or(name).to_owned()
            }
            Target::Page(space) => self
                .session
                .as_ref()
                .and_then(|session| session.pages.iter().find(|(page, ..)| page == space))
                .map(|(_, title, _)| title.clone())
                .unwrap_or_default(),
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
                let old = path.rsplit('/').next().unwrap_or_default();
                if name != old.strip_suffix(".one").unwrap_or(old) {
                    self.commands.push(Command::Structure(
                        library,
                        Structure::Rename { path, name },
                    ));
                }
            }
            Target::Page(space) => {
                if let Err(error) = self.retitle(space, name) {
                    crate::platform::alert("Couldn't rename the page", &error.to_string());
                }
            }
        }
    }

    /// The rename field over the section tab being renamed, of those built as `row` in the
    /// tab row `bar`, where the tab's label stands.
    pub(crate) fn tab_rename_field(&mut self, theme: &Theme, row: Id, bar: Id) {
        let (Some(session), Some(renaming)) = (&self.session, &mut self.renaming) else {
            return;
        };
        let Some(tab) = session
            .tabs
            .iter()
            .position(|tab| renaming.entry(&session.library, &tab.path, true))
        else {
            return;
        };
        let (Some(rect), Some(bar)) =
            (self.ui.rect(ui::shell::tab_id(row, tab)), self.ui.rect(bar))
        else {
            return;
        };
        let width = self.ui.measure(&renaming.name)[0] + 4.0 + 2.0 * PAD;
        self.ui.open(
            "rename",
            Spec {
                flags: ui::Flags::FLOAT,
                size: [px(width.max(48.0)), px(rect[3] - rect[1] - 6.0)],
                position: [
                    rect[0] + ui::shell::TAB_PAD - PAD - bar[0],
                    rect[1] + 3.0 - bar[1],
                ],
                ..Spec::default()
            },
        );
        let kept = edit(
            &mut self.ui,
            theme,
            &mut renaming.name,
            rect[3] - rect[1] - 6.0,
        );
        self.ui.close();
        if let Some(keep) = kept {
            self.finish_renaming(keep);
        }
    }

    /// Gives page `space` of the open section the title `name`.
    fn retitle(&mut self, space: ExGuid, name: String) -> Result<(), Box<dyn Error>> {
        self.persist()?;
        let session = self.session.as_mut().ok_or("No section is open")?;
        let page = session.section.page(space)?;
        let title = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Title(title) => title.outlines.first()?.paragraphs.first()?.text(),
                _ => None,
            })
            .ok_or("The page has no title to rename")?;
        let length = title.text.utf16_offset(title.text.text().len())?;
        session.section.apply(
            &self.author,
            Edit {
                at: crate::filetime(),
                ops: vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text: title.id,
                        range: 0..length,
                        with: name,
                    },
                }],
            },
        )?;
        session.pages = session.section.pages()?;
        self.edited(vec![space]);
        self.refresh()
    }
}
