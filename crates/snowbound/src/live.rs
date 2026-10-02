//! Live presence in the window (feature `live`, on where `SNOWBOUND_LIVE` is `network` or
//! `loopback`): the others with the notebook open as avatars beside the search box, a dot on
//! the tab of the page each has open, and their carets on the page, each in a colour of their
//! own. They meet through the notebook's identity, or through `SNOWBOUND_LIVE_CODE` where it
//! is set (`resources/live-share.md`).

use crate::{Command, Session, State, TAB_ROW, page, platform};
use canvas::{document::TextPosition, editor::TextOutline};
use notebook::live::{self, Caret, Hello, Peer, Reach, Room, Spot};
use onestore::ExGuid;
use std::{collections::HashMap, sync::OnceLock};
use ui::{Flags, Spec, children, fill, fit, px};

/// The side of the picture sent, in pixels.
const PICTURE: u32 = 96;
const AVATAR: f32 = 22.0;
/// The name flag above a caret.
const FLAG: f32 = 15.0;

/// The room joined and the people met there.
pub(crate) struct Peers {
    room: Room,
    /// None where joining failed, which is tried again only for another room.
    live: Option<live::Live>,
    /// Each peer's picture as drawn, cut to a circle, decoded once.
    pictures: HashMap<[u8; 16], Option<draw::RasterImage>>,
}

fn reach() -> Option<Reach> {
    match std::env::var("SNOWBOUND_LIVE").ok()?.as_str() {
        "network" => Some(Reach::Network),
        "loopback" => Some(Reach::Loopback),
        _ => None,
    }
}

impl State {
    /// Joins the open notebook's room, leaving any other, and says where this window is.
    pub(crate) fn follow_peers(&mut self) {
        let Some(reach) = reach() else {
            return;
        };
        let room = match std::env::var("SNOWBOUND_LIVE_CODE") {
            Ok(code) => Some(Room::Code(code)),
            Err(_) => self
                .session
                .as_ref()
                .and_then(|session| session.library.catalog()?.toc.as_ref())
                .map(|toc| Room::Notebook(toc.file_id)),
        };
        if self.peers.as_ref().map(|peers| &peers.room) != room.as_ref() {
            self.peers = room.map(|room| {
                let redraw = self.redraw.clone();
                // The account's picture goes with the account's name, not another chosen in Options.
                let picture = (self.author == platform::user_name())
                    .then(|| account_picture().clone())
                    .flatten();
                let live = Hello::new(self.author.clone(), picture)
                    .and_then(|me| {
                        live::Live::start(me, &room, Some(reach), move || redraw.wake_by_ref())
                    })
                    .inspect_err(|error| eprintln!("Live presence: {error}"))
                    .ok();
                Peers {
                    room,
                    live,
                    pictures: HashMap::new(),
                }
            });
        }
        if let Some(live) = self.peers.as_ref().and_then(|peers| peers.live.as_ref()) {
            live.set_presence(self.presence());
        }
    }

    /// The open section and page, and the caret where it is in stored text.
    fn presence(&self) -> live::Presence {
        let Some(session) = &self.session else {
            return live::Presence::default();
        };
        let outline = self.view.editor.active_outline();
        let [anchor, focus] = self
            .view
            .editor
            .selection()
            .positions
            .map(|position| spot(outline, position));
        live::Presence {
            section: section(session),
            page: Some(session.space.into()),
            caret: anchor
                .zip(focus)
                .map(|(anchor, focus)| Caret { anchor, focus }),
        }
    }

    fn connected(&self) -> Vec<Peer> {
        self.peers
            .as_ref()
            .and_then(|peers| peers.live.as_ref())
            .map(live::Live::peers)
            .unwrap_or_default()
    }

    /// The colours of the peers on each page of the open section.
    pub(crate) fn peer_pages(&self) -> HashMap<ExGuid, Vec<[f32; 4]>> {
        let mut pages: HashMap<ExGuid, Vec<[f32; 4]>> = HashMap::new();
        let Some(session) = &self.session else {
            return pages;
        };
        for peer in self.connected() {
            if let Some(presence) = peer
                .presence
                .filter(|presence| presence.section == section(session))
                && let Some(page) = presence.page
            {
                pages
                    .entry(page.into())
                    .or_default()
                    .push(color(&peer.hello.peer));
            }
        }
        pages
    }

    /// The others with the notebook open, as avatars leftward from the search box: a click
    /// opens the page someone has open in this section.
    pub(crate) fn avatars(&mut self) {
        let peers = self.connected();
        if peers.is_empty() {
            return;
        }
        self.ui.open(
            "peers",
            Spec {
                size: [children(), px(TAB_ROW)],
                pad: [6.0, (TAB_ROW - AVATAR) / 2.0],
                gap: 4.0,
                ..Spec::default()
            },
        );
        for peer in peers.iter().rev() {
            let hello = &peer.hello;
            let picture = self.peers.as_mut().and_then(|peers| {
                peers
                    .pictures
                    .entry(hello.peer)
                    .or_insert_with(|| hello.picture.as_deref().and_then(circle))
                    .clone()
            });
            let avatar = self.ui.open(
                hello.peer,
                Spec {
                    flags: Flags::CLICKABLE,
                    size: [px(AVATAR); 2],
                    fill: Some(color(&hello.peer)),
                    radius: AVATAR / 2.0,
                    pad: [2.0, 2.0],
                    role: Some(accesskit::Role::Button),
                    ..Spec::default()
                },
            );
            match &picture {
                Some(image) => self.ui.leaf(
                    "picture",
                    Spec {
                        size: [fill(), fill()],
                        image: Some(image),
                        ..Spec::default()
                    },
                ),
                None => self.ui.leaf(
                    "initials",
                    Spec {
                        size: [fill(), fill()],
                        text: Some(&initials(&hello.name)),
                        font_size: Some(9.0),
                        bold: true,
                        center: true,
                        color: Some([1.0; 4]),
                        ..Spec::default()
                    },
                ),
            };
            self.ui.close();
            if let Some(node) = self.ui.access(avatar) {
                node.set_label(hello.name.as_str());
            }
            let place = self.place_of(peer);
            ui::popup::tooltip(&mut self.ui, &hello.name, "", place.as_deref());
            if self.ui.signal(avatar).clicked
                && let Some(session) = &self.session
                && let Some(presence) = peer.presence.as_ref()
                && presence.section == section(session)
                && let Some(page) = presence.page
            {
                self.commands.push(Command::OpenPage(page.into()));
            }
        }
        self.ui.close();
    }

    /// The page, or the section, `peer` has open.
    fn place_of(&self, peer: &Peer) -> Option<String> {
        let session = self.session.as_ref()?;
        let presence = peer.presence.as_ref()?;
        if presence.section == section(session) {
            let page = ExGuid::from(presence.page?);
            let (_, title, _) = session.pages.iter().find(|(space, ..)| *space == page)?;
            return Some(if title.is_empty() {
                "Untitled page".into()
            } else {
                title.clone()
            });
        }
        let tab = session.tabs.iter().find(|tab| {
            session.library.section_identity(&tab.path) == presence.section
                && presence.section.is_some()
        })?;
        Some(format!("In {}", tab.name))
    }

    /// Each peer's caret on the open page with a flag naming them, and what they have
    /// selected, as boxes floating in the page's box.
    pub(crate) fn peer_carets(&mut self) {
        let Some(session) = &self.session else {
            return;
        };
        let Some([left, top, right, bottom]) = self.ui.rect(page()) else {
            return;
        };
        let here = (section(session), Some(session.space.into()));
        let scale = self.ui.scale();
        let viewport = self.view.viewport;
        // A point on the page in points from the page box's corner.
        let shown = |[x, y]: [f32; 2]| {
            [
                (x * viewport.scale + viewport.origin[0]) / scale,
                (y * viewport.scale + viewport.origin[1]) / scale,
            ]
        };
        for peer in self.connected() {
            let Some(caret) = peer
                .presence
                .filter(|presence| (presence.section, presence.page) == here)
                .and_then(|presence| presence.caret)
            else {
                continue;
            };
            let Some((outline, focus)) = find(&self.view.editor, caret.focus) else {
                continue;
            };
            let color = color(&peer.hello.peer);
            let [x, y] = outline.origin();
            if let Some((_, anchor)) =
                find(&self.view.editor, caret.anchor).filter(|(other, _)| other.id == outline.id)
            {
                let rects = outline
                    .range_rects([anchor, focus].into())
                    .unwrap_or_default();
                for (index, rect) in rects.into_iter().enumerate() {
                    let [x0, y0] = shown([rect.x0 as f32 + x, rect.y0 as f32 + y]);
                    let [x1, y1] = shown([rect.x1 as f32 + x, rect.y1 as f32 + y]);
                    self.ui.leaf(
                        (peer.hello.peer, "selection", index),
                        Spec {
                            flags: Flags::FLOAT,
                            position: [x0, y0],
                            size: [px(x1 - x0), px(y1 - y0)],
                            fill: Some([color[0], color[1], color[2], 0.25]),
                            ..Spec::default()
                        },
                    );
                }
            }
            let Ok(rect) = outline.caret_at(focus, parley::Affinity::Downstream, 0.0) else {
                continue;
            };
            let [x0, y0] = shown([rect.x0 as f32 + x, rect.y0 as f32 + y]);
            let [_, y1] = shown([rect.x0 as f32 + x, rect.y1 as f32 + y]);
            if !(0.0..right - left).contains(&x0) || y1 < 0.0 || y0 > bottom - top {
                continue;
            }
            self.ui.leaf(
                (peer.hello.peer, "caret"),
                Spec {
                    flags: Flags::FLOAT,
                    position: [x0 - 1.0, y0],
                    size: [px(2.0), px(y1 - y0)],
                    fill: Some(color),
                    ..Spec::default()
                },
            );
            let name = peer
                .hello
                .name
                .split_whitespace()
                .next()
                .unwrap_or("Someone");
            self.ui.leaf(
                (peer.hello.peer, "flag"),
                Spec {
                    flags: Flags::FLOAT,
                    position: [x0 - 1.0, y0 - FLAG],
                    size: [fit(), px(FLAG)],
                    text: Some(name),
                    font_size: Some(10.0),
                    bold: true,
                    color: Some([1.0; 4]),
                    fill: Some(color),
                    radius: 3.0,
                    pad: [4.0, 0.0],
                    ..Spec::default()
                },
            );
        }
    }
}

/// The open section's file identity.
fn section(session: &Session) -> Option<[u8; 16]> {
    session
        .library
        .section_identity(&session.tabs[session.tab].path)
}

/// `position` in `outline` as ops address it: its text object and offset.
fn spot(outline: &TextOutline, position: TextPosition) -> Option<Spot> {
    let text = outline
        .document()
        .text_nodes()
        .nth(position.paragraph)?
        .text()?;
    Some(Spot {
        text: text.id.into(),
        offset: position.offset,
    })
}

/// The outline on the page holding `spot`, and where in it.
fn find(editor: &canvas::editor::CanvasEditor, spot: Spot) -> Option<(&TextOutline, TextPosition)> {
    let text = ExGuid::from(spot.text);
    editor.visible_outlines().find_map(|outline| {
        let paragraph = outline
            .document()
            .text_nodes()
            .position(|node| node.text().is_some_and(|object| object.id == text))?;
        Some((
            outline,
            TextPosition {
                paragraph,
                offset: spot.offset,
            },
        ))
    })
}

/// A peer's colour, the same in every window that shows them.
fn color(peer: &[u8; 16]) -> [f32; 4] {
    const COLORS: [[u8; 3]; 8] = [
        [0x1a, 0x73, 0xe8],
        [0xd9, 0x30, 0x25],
        [0x18, 0x80, 0x38],
        [0xe3, 0x74, 0x00],
        [0x93, 0x34, 0xe6],
        [0x00, 0x89, 0x7b],
        [0xd0, 0x1b, 0x8c],
        [0x80, 0x5a, 0x2e],
    ];
    let [red, green, blue] = COLORS[usize::from(peer[0]) % COLORS.len()];
    draw::srgb(red, green, blue)
}

/// The first letters of a name's first and last words.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let first = |word: Option<&&str>| word.and_then(|word| word.chars().next());
    [
        first(words.first()),
        first(words.last()).filter(|_| words.len() > 1),
    ]
    .into_iter()
    .flatten()
    .flat_map(char::to_uppercase)
    .collect()
}

/// `picture`'s middle square, cut to a circle.
fn circle(picture: &[u8]) -> Option<draw::RasterImage> {
    let image = draw::RasterImage::decode(picture, [PICTURE; 2]).ok()?;
    let [width, height] = image.size();
    let side = width.min(height);
    let [dx, dy] = [(width - side) / 2, (height - side) / 2];
    let radius = side as f32 / 2.0;
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let at = (((y + dy) * width + x + dx) * 4) as usize;
            let pixel = &image.pixels()[at..at + 4];
            let distance = (x as f32 + 0.5 - radius).hypot(y as f32 + 0.5 - radius);
            let coverage = (radius - distance + 0.5).clamp(0.0, 1.0);
            pixels.extend_from_slice(&pixel[..3]);
            pixels.push((f32::from(pixel[3]) * coverage) as u8);
        }
    }
    draw::RasterImage::new([side; 2], pixels).ok()
}

/// The account's picture, as the system shows it at sign-in, as a PNG at most `PICTURE`
/// pixels a side.
fn account_picture() -> &'static Option<Vec<u8>> {
    static PNG: OnceLock<Option<Vec<u8>>> = OnceLock::new();
    PNG.get_or_init(|| {
        let image = draw::RasterImage::decode(&system_picture()?, [PICTURE; 2]).ok()?;
        let [width, height] = image.size();
        let mut png = Vec::new();
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        // Decoding premultiplies, which leaves an opaque photo as it was.
        encoder
            .write_header()
            .ok()?
            .write_image_data(image.pixels())
            .ok()?;
        Some(png)
    })
}

/// Directory Services keeps the picture as hex under `JPEGPhoto`, or names a file.
#[cfg(target_os = "macos")]
fn system_picture() -> Option<Vec<u8>> {
    let user = format!("/Users/{}", std::env::var("USER").ok()?);
    let read = |attribute: &str| {
        let output = std::process::Command::new("/usr/bin/dscl")
            .args([".", "-read", &user, attribute])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        Some(text.split_once(':')?.1.trim().to_owned())
    };
    if let Some(hex) = read("JPEGPhoto") {
        let digits: Vec<u8> = hex.bytes().filter(u8::is_ascii_hexdigit).collect();
        let bytes: Option<Vec<u8>> = digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
            .collect();
        if let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) {
            return Some(bytes);
        }
    }
    notebook::fs::read(read("Picture")?).ok()
}

/// Where desktops keep it: `~/.face`, or AccountsService's icon for the account.
#[cfg(target_os = "linux")]
fn system_picture() -> Option<Vec<u8>> {
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let user = std::env::var("USER").unwrap_or_default();
    [
        home.join(".face"),
        home.join(".face.icon"),
        std::path::Path::new("/var/lib/AccountsService/icons").join(user),
    ]
    .into_iter()
    .find_map(|path| notebook::fs::read(path).ok())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn system_picture() -> Option<Vec<u8>> {
    None
}
