//! File, Print… and Export as PDF…: the open page, or the whole section, laid on paper as
//! OneNote 2010 prints it (`canvas::print`), then handed to the system's printing or saved.

use crate::{State, UserEvent, platform, printer};
use onestore::page::Page;
use std::{error::Error, path::PathBuf, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Page,
    Section,
}

impl State {
    /// Prints `scope` through the system's print dialog, or with `export` saves it as a PDF
    /// where the user chooses. Pages are laid out on a thread of their own.
    pub(crate) fn print(&mut self, scope: Scope, export: bool) -> Result<(), Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let section = session.tabs[session.tab].name.clone();
        // The open page as the editor holds it; the section's others as stored.
        let open = (session.space, self.view.editor.page()?);
        let spaces: Vec<_> = match scope {
            Scope::Page => vec![session.space],
            Scope::Section => session.pages.iter().map(|(space, ..)| *space).collect(),
        };
        let title = match scope {
            Scope::Page => open.1.title.clone(),
            Scope::Section => section.clone(),
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
        let replica = Arc::clone(session.section.replica());
        let layouts = Arc::clone(&self.layouts);
        let proxy = self.proxy.clone();
        let paper = printer::paper();
        std::thread::spawn(move || {
            let made = (|| -> Result<Vec<u8>, Box<dyn Error>> {
                let mut open = Some(open);
                let pages = spaces
                    .into_iter()
                    .map(|space| match open.take_if(|(shown, _)| *shown == space) {
                        Some((_, page)) => Ok(page),
                        None => replica.page(space).map_err(Into::into),
                    })
                    .collect::<Result<Vec<Page>, Box<dyn Error>>>()?;
                let mut engine = layouts.lock().map_err(|_| "Page layout failed")?;
                Ok(canvas::print::pdf(pages, &mut engine, paper, &section)?)
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
}
