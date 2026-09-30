//! File, Print… and Export as PDF…: OneNote 2010's Print Preview and Settings, less the
//! preview, then the chosen pages laid on paper as OneNote prints them (`canvas::print`),
//! handed to the system's printing or saved.

use crate::{State, UserEvent, library::Library, platform, printer};
use accesskit::Role;
use canvas::print::Footer;
use notebook::discover::Folder;
use onestore::{ExGuid, page::Page};
use std::{error::Error, path::PathBuf, sync::Arc};
use ui::{Anchor, Axis, Id, Spec, Ui, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Page,
    /// The page with the subpages of its group.
    PageGroup,
    Section,
    /// Every section of the notebook, which only Export as PDF offers, as OneNote's Save As.
    Notebook,
}

const SCOPES: [(Scope, &str); 4] = [
    (Scope::Page, "Current Page"),
    (Scope::PageGroup, "Page Group"),
    (Scope::Section, "Current Section"),
    (Scope::Notebook, "Current Notebook"),
];

/// Print Preview's paper sizes, upright, in points.
const PAPERS: [(&str, [f32; 2]); 8] = [
    ("Letter", canvas::print::LETTER),
    ("Tabloid", [792.0, 1224.0]),
    ("Legal", [612.0, 1008.0]),
    ("Executive", [522.0, 756.0]),
    ("A3", [841.89, 1190.55]),
    ("A4", canvas::print::A4),
    ("B4 (JIS)", [728.5, 1031.81]),
    ("B5 (JIS)", [515.91, 728.5]),
];

const FOOTERS: [Footer; 4] = [
    Footer::SectionAndPage,
    Footer::Page,
    Footer::Section,
    Footer::None,
];

/// macOS's print panel chooses the paper, and the sheets are fitted to it.
const PRINT_PAPER: bool = !cfg!(target_os = "macos");

/// What to print and how, as Print Preview and Settings offers it.
#[derive(Clone, Copy)]
pub struct Setup {
    scope: Scope,
    /// Upright width and height.
    paper: [f32; 2],
    landscape: bool,
    fit_width: bool,
    footer: Footer,
}

/// The choices last made, and the dialog's while it is open.
#[derive(Default)]
pub struct Printing {
    last: Option<Setup>,
    dialog: Option<Dialog>,
}

struct Dialog {
    export: bool,
    setup: Setup,
}

impl Printing {
    pub fn open(&self) -> bool {
        self.dialog.is_some()
    }
}

fn id() -> Id {
    Id::ROOT.child("print")
}

/// A combo's value and the menu listing `names`, choosing among them.
fn choose(ui: &mut Ui, name: &str, names: &[&str], current: usize) -> Option<usize> {
    let menu = id().child(name);
    let combo = ui.id(name);
    let shown = names.get(current).copied().unwrap_or("Other");
    ui::shell::combo(ui, name, name, shown, 180.0, menu, true);
    let items: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(at, text)| Item {
            text,
            checked: Some(at == current),
            current: at == current,
            ..Item::default()
        })
        .collect();
    let anchor = Anchor::Below(ui.rect(combo).unwrap_or_default());
    ui::popup::menu(ui, menu, anchor, &items, None)
}

/// A row of `label` and the controls `build` adds after it.
fn field(ui: &mut Ui, label: &str, build: impl FnOnce(&mut Ui)) {
    let row = ui.theme.font_size * 2.0;
    ui.open(
        label,
        Spec {
            size: [fill(), px(row)],
            gap: 8.0,
            ..Spec::default()
        },
    );
    ui.leaf(
        "label",
        Spec {
            size: [px(110.0), px(row)],
            text: Some(label),
            ..Spec::default()
        },
    );
    build(ui);
    ui.close();
}

impl State {
    /// Opens Print Preview and Settings for Print…, or with `export` for Export as PDF….
    pub(crate) fn open_print(&mut self, export: bool) {
        let mut setup = self.printing.last.unwrap_or_else(|| Setup {
            scope: Scope::Page,
            paper: printer::paper(),
            landscape: false,
            fit_width: true,
            footer: Footer::SectionAndPage,
        });
        if !export && setup.scope == Scope::Notebook {
            setup.scope = Scope::Page;
        }
        if !export && !PRINT_PAPER {
            setup.paper = printer::paper();
        }
        self.printing.dialog = Some(Dialog { export, setup });
        self.ui.open_popup(id());
    }

    /// Builds the dialog while it is open; Print or Export goes on with its choices, which
    /// the next opening starts from.
    pub(crate) fn print_dialog(&mut self) {
        let Some(dialog) = &mut self.printing.dialog else {
            return;
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.printing.dialog = None;
            return;
        }
        let section = self.session.as_ref().map_or(String::new(), |session| {
            session.tabs[session.tab].name.clone()
        });
        let theme = ui.theme.clone();
        let entered = ui::popup::navigation(ui, &[], &[NamedKey::Enter]).contains(&NamedKey::Enter);
        let title = if dialog.export {
            "Export as PDF"
        } else {
            "Print"
        };
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(340.0), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 6.0,
                anchor: Some(Anchor::Dialog),
                role: Some(Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label(title);
        }
        ui.leaf(
            "title",
            Spec {
                size: [fill(), px(theme.font_size * 2.0)],
                text: Some(title),
                bold: true,
                role: Some(Role::Heading),
                ..Spec::default()
            },
        );
        let setup = &mut dialog.setup;
        let scopes = if dialog.export { 4 } else { 3 };
        let names: Vec<&str> = SCOPES[..scopes].iter().map(|(_, name)| *name).collect();
        field(ui, "Print range:", |ui| {
            let current = SCOPES.iter().position(|(scope, _)| *scope == setup.scope);
            if let Some(at) = choose(ui, "Print range", &names, current.unwrap_or(0)) {
                setup.scope = SCOPES[at].0;
            }
        });
        if dialog.export || PRINT_PAPER {
            let names = PAPERS.map(|(name, _)| name);
            field(ui, "Paper size:", |ui| {
                let current = PAPERS.iter().position(|(_, size)| {
                    size.iter()
                        .zip(setup.paper)
                        .all(|(a, b)| (a - b).abs() < 1.0)
                });
                if let Some(at) = choose(ui, "Paper size", &names, current.unwrap_or(usize::MAX)) {
                    setup.paper = PAPERS[at].1;
                }
            });
        }
        field(ui, "Orientation:", |ui| {
            let names = ["Portrait", "Landscape"];
            if let Some(at) = choose(ui, "Orientation", &names, usize::from(setup.landscape)) {
                setup.landscape = at == 1;
            }
        });
        if ui::check_box(
            ui,
            "fit-width",
            "Scale content to paper width",
            setup.fit_width,
        )
        .clicked
        {
            setup.fit_width = !setup.fit_width;
        }
        let footers = [
            format!("{section} Page (number)"),
            "Page (number)".to_owned(),
            section.clone(),
            "(none)".to_owned(),
        ];
        let names: Vec<&str> = footers.iter().map(String::as_str).collect();
        field(ui, "Footer:", |ui| {
            let current = FOOTERS.iter().position(|footer| *footer == setup.footer);
            if let Some(at) = choose(ui, "Footer", &names, current.unwrap_or(0)) {
                setup.footer = FOOTERS[at];
            }
        });
        ui.open(
            "buttons",
            Spec {
                size: [fill(), children()],
                pad: [0.0, 8.0],
                gap: 8.0,
                ..Spec::default()
            },
        );
        ui.leaf(
            "space",
            Spec {
                size: [fill(), px(1.0)],
                ..Spec::default()
            },
        );
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        let action = if dialog.export {
            "Export…"
        } else {
            "Print…"
        };
        let go = ui::button(ui, "go", action).clicked || entered;
        ui.close();
        ui.close();
        if !go && !cancel {
            return;
        }
        let Dialog { export, setup } = self.printing.dialog.take().expect("The dialog is open");
        self.ui.close_popup(id());
        if go {
            self.printing.last = Some(setup);
            if let Err(error) = self.print(setup, export) {
                platform::alert("Couldn't print", &error.to_string());
            }
        }
    }

    /// Prints `setup`'s pages through the system's print dialog, or with `export` saves
    /// them as a PDF where the user chooses. Pages are read and laid out on a thread of
    /// their own.
    fn print(&mut self, setup: Setup, export: bool) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let tab = &session.tabs[session.tab];
        // The open page as the editor holds it; the others as stored.
        let open = (session.space, self.view.editor.page()?);
        let at = session
            .pages
            .iter()
            .position(|(space, ..)| *space == session.space);
        let spaces: Vec<ExGuid> = match (setup.scope, at) {
            (Scope::PageGroup, Some(at)) => {
                let levels: Vec<u32> = session.pages.iter().map(|(.., level)| *level).collect();
                session.pages[group(&levels, at)]
                    .iter()
                    .map(|(space, ..)| *space)
                    .collect()
            }
            (Scope::Section | Scope::Notebook, _) => {
                session.pages.iter().map(|(space, ..)| *space).collect()
            }
            _ => vec![session.space],
        };
        let title = match setup.scope {
            Scope::Page | Scope::PageGroup => open.1.title.clone(),
            Scope::Section => tab.name.clone(),
            Scope::Notebook => session.library.name.clone(),
        };
        let destination = if export {
            let name = format!("{}.pdf", file_name(&title));
            let Some(mut path) = platform::pick_new("Export as PDF", &name, "Export", None) else {
                return Ok(());
            };
            if path.extension().is_none() {
                path.set_extension("pdf");
            }
            Some(path)
        } else {
            None
        };
        // Other sections are opened on the thread, in the notebook's order.
        let others = match setup.scope {
            Scope::Notebook => notebook_tabs(&session.library),
            _ => Vec::new(),
        };
        let (open_tab, library) = (
            (tab.path.clone(), tab.name.clone()),
            Arc::clone(&session.library),
        );
        let replica = Arc::clone(session.section.replica());
        let layouts = Arc::clone(&self.layouts);
        let proxy = self.proxy.clone();
        let paper = if setup.landscape {
            [setup.paper[1], setup.paper[0]]
        } else {
            setup.paper
        };
        let layout = canvas::print::Setup {
            paper,
            fit_width: setup.fit_width,
            footer: setup.footer,
        };
        std::thread::spawn(move || {
            let made = (|| -> Result<Vec<u8>, Box<dyn Error>> {
                let mut open = Some(open);
                let this = spaces
                    .into_iter()
                    .map(|space| match open.take_if(|(shown, _)| *shown == space) {
                        Some((_, page)) => Ok(page),
                        None => replica.page(space).map_err(Into::into),
                    })
                    .collect::<Result<Vec<Page>, Box<dyn Error>>>()?;
                let mut this = Some((open_tab.1.clone(), this));
                let sections = if others.is_empty() {
                    this.into_iter().collect()
                } else {
                    others
                        .into_iter()
                        .map(|(path, name)| {
                            if path == open_tab.0
                                && let Some(open) = this.take()
                            {
                                return Ok(open);
                            }
                            let section = library.open(&path, || {})?;
                            let pages = section
                                .pages()?
                                .into_iter()
                                .map(|(space, ..)| section.page(space))
                                .collect::<Result<Vec<_>, _>>()?;
                            Ok((name, pages))
                        })
                        .collect::<Result<Vec<_>, Box<dyn Error>>>()?
                };
                let mut engine = layouts.lock().map_err(|_| "Page layout failed")?;
                Ok(canvas::print::pdf(sections, &mut engine, &layout)?)
            })();
            let done = made.and_then(|pdf| match &destination {
                Some(path) => std::fs::write(path, pdf).map(|()| None).map_err(Into::into),
                None => Ok(Some(pdf)),
            });
            let failed = if export {
                "Couldn't export the PDF"
            } else {
                "Couldn't print"
            };
            let done = done.map_err(|error| error.to_string());
            let _ = proxy.send_event(UserEvent::Then(Box::new(move |state| {
                match done {
                    Ok(Some(pdf)) => printer::print(&state.window, &pdf, &title),
                    Ok(None) => Ok(()),
                    Err(error) => Err(error.into()),
                }
                .inspect_err(|error| platform::alert(failed, &error.to_string()))
            })));
        });
        Ok(())
    }
}

/// The pages of the group the page at `at` is in, by the pages' outline `levels`: the top
/// page before it, or itself, and the subpages after that.
fn group(levels: &[u32], at: usize) -> std::ops::Range<usize> {
    let top = |level: &u32| *level <= 1;
    let start = levels[..=at].iter().rposition(top).unwrap_or(at);
    let end = levels[start + 1..]
        .iter()
        .position(top)
        .map_or(levels.len(), |end| start + 1 + end);
    start..end
}

/// Every readable section of `library`, catalog path and name, a folder's sections before
/// its groups', as OneNote orders them.
fn notebook_tabs(library: &Library) -> Vec<(String, String)> {
    fn walk(library: &Library, folder: &Folder, tabs: &mut Vec<(String, String)>) {
        tabs.extend(
            library
                .tabs(&folder.path)
                .into_iter()
                .map(|tab| (tab.path, tab.name)),
        );
        for group in &folder.groups {
            walk(library, group, tabs);
        }
    }
    let mut tabs = Vec::new();
    match library.catalog() {
        Some(catalog) => walk(library, catalog, &mut tabs),
        None => tabs.extend(library.tabs("").into_iter().map(|tab| (tab.path, tab.name))),
    }
    tabs
}

/// `title` as a file name: without the characters Windows and macOS refuse in one.
fn file_name(title: &str) -> String {
    let name: String = title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    match name.trim() {
        "" => "Untitled".to_owned(),
        name => name.to_owned(),
    }
}

/// A PDF waiting for a system viewer or print service, in the cache folder under `title`.
pub(crate) fn spooled(pdf: &[u8], title: &str) -> Result<PathBuf, Box<dyn Error>> {
    let folder = platform::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("Printing");
    std::fs::create_dir_all(&folder)?;
    let path = folder.join(format!("{}.pdf", file_name(title)));
    std::fs::write(&path, pdf)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_file_names() {
        assert_eq!(file_name("Notes: 3/4"), "Notes- 3-4");
        assert_eq!(file_name("  "), "Untitled");
    }

    #[test]
    fn a_page_group_is_its_top_page_and_subpages() {
        let levels = [1, 2, 3, 1, 1, 2];
        assert_eq!(group(&levels, 2), 0..3);
        assert_eq!(group(&levels, 0), 0..3);
        assert_eq!(group(&levels, 3), 3..4);
        assert_eq!(group(&levels, 5), 4..6);
    }
}
