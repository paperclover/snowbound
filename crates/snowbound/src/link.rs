//! Links and equations on the page as OneNote 2010 offers them: the Link dialog,
//! the page's context menu, and following a clicked link, to a page
//! of the notebook or with the system's handler.

use crate::{Command, State, page, platform};
use onestore::ExGuid;
use onestore::page::link::{LinkTarget, internal_link};
use std::error::Error;
use ui::{Anchor, Axis, Flags, Id, Spec, children, fill, popup::Item, px};
use winit::keyboard::NamedKey;

const WIDTH: f32 = 420.0;
/// Pages the dialog lists at once; the search narrows the rest.
const PAGES: usize = 8;

/// The Link dialog's fields while it is open.
pub struct LinkDialog {
    text: String,
    address: String,
    search: String,
}

fn id() -> Id {
    Id::ROOT.child("link")
}

fn text_field() -> Id {
    id().child("text")
}

fn address_field() -> Id {
    id().child("address")
}

fn search_field() -> Id {
    id().child("search")
}

fn menu() -> Id {
    Id::ROOT.child("text-menu")
}

/// What a link's address opens with the system's handler: a web address typed without its
/// scheme is a web page, a share's `\\server\share` path its `smb:` URL.
fn system_url(address: &str) -> String {
    if let Some(share) = address.strip_prefix("\\\\") {
        return format!("smb://{}", share.replace('\\', "/"));
    }
    let scheme = address.split_once(':').is_some_and(|(scheme, _)| {
        scheme.len() > 1
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    if scheme {
        address.to_owned()
    } else {
        format!("http://{address}")
    }
}

/// A page, or a paragraph on it, as Copy Link to Page gives it: the section file's path
/// before the title, then the identities OneNote finds it by.
fn clipboard_link(
    path: &str,
    section: [u8; 16],
    title: &str,
    page: [u8; 16],
    object: Option<ExGuid>,
) -> String {
    let target = match object {
        Some(object) => LinkTarget::Object {
            identity: page,
            title,
            object,
        },
        None => LinkTarget::Page {
            identity: page,
            title,
        },
    };
    let stored = internal_link(section, "", target);
    let fragment = stored
        .strip_prefix("onenote:#")
        .and_then(|rest| rest.strip_suffix("&base-path="))
        .unwrap_or_default();
    format!("onenote:///{}#{fragment}", path.replace(' ', "%20"))
}

impl State {
    /// Ctrl+K and the toolbar's Link: the dialog opens on what the selection links.
    pub(crate) fn open_link_dialog(&mut self) {
        // As OneNote 2010's, Link does nothing over a selection across paragraphs.
        let Some((text, address)) = self.view.link_prefill() else {
            return;
        };
        self.link = Some(LinkDialog {
            text,
            address,
            search: String::new(),
        });
        self.ui.open_popup(id());
        self.ui.set_focus(Some(address_field()));
    }

    /// Builds the Link dialog while it is open. OK, or Enter in a field, links the address
    /// shown as the text; a page picked from the list fills both.
    pub(crate) fn link_dialog(&mut self) -> Result<(), Box<dyn Error>> {
        let Some(dialog) = &mut self.link else {
            return Ok(());
        };
        let ui = &mut self.ui;
        if !ui.popup_open(id()) {
            self.link = None;
            return Ok(());
        }
        let theme = ui.theme.clone();
        let row = theme.font_size * 2.0;
        let fields = [text_field(), address_field(), search_field()];
        let entered =
            ui::popup::navigation(ui, &fields[..2], &[NamedKey::Enter]).contains(&NamedKey::Enter);
        ui.open_as(
            id(),
            Spec {
                axis: Axis::Y,
                size: [px(WIDTH), children()],
                fill: Some(theme.popup),
                border: Some(theme.chip),
                shadow: Some(theme.shadow),
                radius: 8.0,
                pad: [16.0, 12.0],
                gap: 4.0,
                anchor: Some(Anchor::Dialog),
                role: Some(accesskit::Role::Dialog),
                ..Spec::default()
            },
        );
        if let Some(node) = ui.access(id()) {
            node.set_label("Link");
        }
        let field = |ui: &mut ui::Ui, label: &str, id: Id, value: &mut String, hint: &str| {
            if !label.is_empty() {
                ui.leaf(
                    label,
                    Spec {
                        size: [fill(), px(row * 0.8)],
                        text: Some(label),
                        ..Spec::default()
                    },
                );
            }
            ui::text_field(
                ui,
                id,
                value,
                hint,
                Spec {
                    size: [fill(), px(row)],
                    fill: Some(theme.base),
                    border: Some(theme.accent),
                    radius: 4.0,
                    pad: [6.0, 0.0],
                    ..Spec::default()
                },
            );
            if let Some(node) = ui.access(id) {
                node.set_label(label.trim_end_matches(':'));
            }
        };
        field(ui, "Text to display:", text_field(), &mut dialog.text, "");
        field(ui, "Address:", address_field(), &mut dialog.address, "");
        field(
            ui,
            "Or pick a location in OneNote:",
            search_field(),
            &mut dialog.search,
            "Search by text in title",
        );
        let search = dialog.search.to_lowercase();
        let pages: Vec<(ExGuid, String)> = self
            .session
            .as_ref()
            .map(|session| {
                session
                    .pages
                    .iter()
                    .filter(|(_, title, _)| title.to_lowercase().contains(&search))
                    .take(PAGES)
                    .map(|(space, title, _)| (*space, title.clone()))
                    .collect()
            })
            .unwrap_or_default();
        ui.open(
            "pages",
            Spec {
                axis: Axis::Y,
                size: [fill(), px(row * PAGES as f32 + 8.0)],
                fill: Some(theme.base),
                border: Some(theme.chip),
                radius: 4.0,
                pad: [4.0, 4.0],
                flags: Flags::CLIP,
                role: Some(accesskit::Role::List),
                ..Spec::default()
            },
        );
        let mut picked = None;
        for (space, title) in &pages {
            let shown = if title.is_empty() {
                "Untitled page"
            } else {
                title
            };
            let spec = Spec {
                flags: Flags::CLICKABLE,
                size: [fill(), px(row)],
                text: Some(shown),
                hover_fill: Some(theme.hover()),
                radius: 4.0,
                pad: [8.0, 0.0],
                role: Some(accesskit::Role::ListItem),
                ..Spec::default()
            };
            if ui.leaf(space, spec).clicked {
                picked = Some((*space, title.clone()));
            }
        }
        ui.close();
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
        let ok = ui::button(ui, "ok", "OK").clicked || entered;
        let cancel = ui::button(ui, "cancel", "Cancel").clicked;
        ui.close();
        ui.close();
        if let Some((space, title)) = picked
            && let Some(session) = &self.session
        {
            let page = session.section.page(space)?;
            let identity = page
                .identity
                .ok_or("This page has no identity to link to")?;
            let address = internal_link(
                session.section.identity()?,
                &session.section.file().to_string_lossy(),
                LinkTarget::Page {
                    identity,
                    title: &title,
                },
            );
            let dialog = self.link.as_mut().unwrap();
            dialog.address = address;
            if dialog.text.is_empty() {
                dialog.text = title;
            }
            return Ok(());
        }
        let dialog = self.link.as_ref().unwrap();
        if ok && !dialog.address.trim().is_empty() {
            let response = self.view.set_link(&dialog.text, &dialog.address)?;
            self.respond(response);
        } else if !cancel {
            return Ok(());
        }
        self.ui.close_popup(id());
        self.ui.set_focus(Some(page()));
        self.link = None;
        Ok(())
    }

    /// A secondary press on the page's text or a file opens its context menu there.
    pub(crate) fn open_text_menu(&mut self) -> Result<(), Box<dyn Error>> {
        let Some((response, context)) = self.view.context()? else {
            return Ok(());
        };
        self.respond(response);
        let point = self.ui.pointer().unwrap_or_default();
        self.text_menu = Some((context, point));
        self.ui.open_popup(menu());
        Ok(())
    }

    /// The page's context menu while it is open, with OneNote 2010's commands for text,
    /// links, equations and files.
    pub(crate) fn text_menu(&mut self) -> Result<(), Box<dyn Error>> {
        let Some((context, point)) = &self.text_menu else {
            return Ok(());
        };
        if !self.ui.popup_open(menu()) {
            self.text_menu = None;
            return Ok(());
        }
        let item = |text| Item {
            text,
            ..Item::default()
        };
        // OneNote 2010 heads the menu on a marked word with its corrections.
        let mut items = match &context.spelling {
            Some(correction) if correction.repeated => vec![
                item("Delete Repeated Word"),
                item("Ignore"),
                Item {
                    separated: true,
                    ..item("Spelling…")
                },
            ],
            Some(correction) => {
                let mut items: Vec<Item> = correction
                    .suggestions
                    .iter()
                    .map(|suggestion| item(suggestion.as_str()))
                    .collect();
                if items.is_empty() {
                    items.push(Item {
                        disabled: true,
                        ..item("(No Spelling Suggestions)")
                    });
                }
                items.extend([
                    Item {
                        separated: true,
                        ..item("Ignore")
                    },
                    item("Add to Dictionary"),
                    Item {
                        separated: true,
                        ..item("Spelling…")
                    },
                ]);
                items
            }
            None => Vec::new(),
        };
        let corrections = items.len();
        items.extend(if context.attachment.is_some() {
            // OneNote 2010's commands for the file itself; its clipboard holds text alone.
            vec![item("Open"), item("Save As…")]
        } else {
            let mut items = vec![
                Item {
                    disabled: !context.selected,
                    separated: corrections > 0,
                    ..item("Cut")
                },
                Item {
                    disabled: !context.selected,
                    ..item("Copy")
                },
                item("Paste"),
            ];
            match &context.link {
                Some(_) => items.extend([
                    Item {
                        separated: true,
                        ..item("Edit Link…")
                    },
                    item("Copy Link to Paragraph"),
                    Item {
                        separated: true,
                        ..item("Copy Link")
                    },
                    item("Select Link"),
                    item("Remove Link"),
                ]),
                None => items.extend([
                    Item {
                        separated: true,
                        ..item("Link…")
                    },
                    item("Copy Link to Paragraph"),
                ]),
            }
            if context.equation {
                items.extend([
                    Item {
                        separated: true,
                        ..item("Professional")
                    },
                    item("Linear"),
                ]);
            }
            items
        });
        let Some(chosen) =
            ui::popup::menu(&mut self.ui, menu(), Anchor::Point(*point), &items, None)
        else {
            return Ok(());
        };
        let chosen_text = items[chosen].text.to_owned();
        let Some((context, _)) = self.text_menu.take() else {
            return Ok(());
        };
        if chosen < corrections {
            let correction = context.spelling.unwrap();
            let response = match chosen_text.as_str() {
                _ if chosen < correction.suggestions.len() => {
                    self.view.correct(&correction, &chosen_text)?
                }
                "Delete Repeated Word" => self.view.correct(&correction, "")?,
                "Spelling…" => {
                    self.open_spelling_pane();
                    return Ok(());
                }
                text => {
                    if let Some(spelling) = &self.view.spelling {
                        if text == "Ignore" {
                            spelling.ignore(&correction.word);
                        } else {
                            spelling.learn(&correction.word);
                        }
                    }
                    self.window.request_redraw();
                    return Ok(());
                }
            };
            self.respond(response);
            return Ok(());
        }
        let response = match chosen_text.as_str() {
            "Open" => return self.open_attachment(&context.attachment.unwrap()),
            "Save As…" => return self.save_attachment(&context.attachment.unwrap()),
            "Cut" => self.view.copy(true)?,
            "Copy" => self.view.copy(false)?,
            "Paste" => {
                self.commands
                    .push(Command::Page(canvas::interaction::Request::Paste));
                return Ok(());
            }
            "Edit Link…" | "Link…" => {
                self.open_link_dialog();
                return Ok(());
            }
            "Copy Link" => {
                self.clipboard.set_text(context.link.unwrap_or_default())?;
                return Ok(());
            }
            "Copy Link to Paragraph" => {
                if let Some(session) = &self.session {
                    let link = self.page_link(session.space, context.paragraph)?;
                    self.clipboard.set_text(link)?;
                }
                return Ok(());
            }
            "Select Link" => self.view.unlink(true)?,
            "Remove Link" => self.view.unlink(false)?,
            text => self.view.switch_equation(text == "Linear")?,
        };
        self.respond(response);
        Ok(())
    }

    /// Copy Link to Page, or to a paragraph `object` on it: the link OneNote puts on the
    /// clipboard for page `space` of the open section.
    pub(crate) fn page_link(
        &self,
        space: ExGuid,
        object: Option<ExGuid>,
    ) -> Result<String, Box<dyn Error>> {
        let session = self.session.as_ref().ok_or("No section is open")?;
        let page = session.section.page(space)?;
        let identity = page
            .identity
            .ok_or("This page has no identity to link to")?;
        let title = session
            .pages
            .iter()
            .find(|(listed, ..)| *listed == space)
            .map_or(page.title.as_str(), |(_, title, _)| title.as_str());
        Ok(clipboard_link(
            &session.section.file().to_string_lossy(),
            session.section.identity()?,
            title,
            identity,
            object,
        ))
    }

    /// A clicked link: a page or section of an open notebook opens here; anything else opens
    /// with the system's handler.
    pub(crate) fn open_link(&mut self, address: &str) -> Result<(), Box<dyn Error>> {
        if !address.to_ascii_lowercase().starts_with("onenote:") {
            platform::reveal(system_url(address));
            return Ok(());
        }
        let Some(session) = &self.session else {
            return Ok(());
        };
        let library = std::sync::Arc::clone(&session.library);
        let Ok(Some(notebook)) = &library.notebook else {
            return Ok(());
        };
        let Some((path, space)) = notebook.find_page(address)? else {
            return Ok(());
        };
        if path == session.tabs[session.tab].path {
            self.commands.extend(space.map(Command::OpenPage));
            return Ok(());
        }
        if let Some(space) = space {
            self.last_pages.insert(library.key(&path), space);
        }
        self.commands.push(Command::OpenSection(library, path));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_open_as_onenote_follows_them() {
        assert_eq!(system_url("www.example.com"), "http://www.example.com");
        assert_eq!(system_url("example.net/x"), "http://example.net/x");
        assert_eq!(system_url("https://a.example/p"), "https://a.example/p");
        assert_eq!(system_url("mailto:me@example.com"), "mailto:me@example.com");
        assert_eq!(system_url("\\\\server\\share\\f"), "smb://server/share/f");
    }

    /// OneNote 2010's Copy Link to Page (`corpus/link-edit/native-typed`, README): the file's
    /// path, the title, and the section and page identities.
    #[test]
    fn page_links_copy_as_onenote_copies_them() {
        let section = *b"\x25\xfc\x1e\x0f\x7c\xdf\xcb\x45\x87\xa4\xbd\xd0\x3f\x42\x05\x7e";
        let page = *b"\x18\xa3\xcc\xa9\x3e\xa4\x12\x49\xa1\xc7\xf1\x9f\x1e\xaf\x30\xca";
        assert_eq!(
            clipboard_link("C:\\one\\links.one", section, "Links", page, None),
            "onenote:///C:\\one\\links.one#Links&section-id={0F1EFC25-DF7C-45CB-87A4-BDD03F42057E}&page-id={A9CCA318-A43E-4912-A1C7-F19F1EAF30CA}&end"
        );
    }
}
