//! Devices sharing a section through a cloud drive that, like iCloud Drive, syncs whole files
//! with no lock and no compare-and-swap: an upload made from a copy the server has moved past
//! makes one of the two the file and keeps the other as a conflict version, which every device
//! sees. Each device commits to its own copy through its replica and merges the conflict
//! versions it sees (`Remote::versions`). Random schedules of typing, new paragraphs, new,
//! deleted and moved pages, offline spans and late deliveries must end with every device on
//! the same file, holding each typed string once.

#[path = "../../onestore/tests/support/sweep.rs"]
mod sweep;

use notebook::{Remote, Replica, Version};
use onestore::{
    CommitError, CommitState, ExGuid, PageCreation, PageEdit, Section, Stamp, Transaction,
    document::{Format, Layout},
    op::{Edit, Op, PageOp, SectionOp, lower_page},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::{collections::BTreeSet, io};

/// The file as the server holds it, and the versions it keeps beside it.
struct Server {
    current: Vec<u8>,
    /// Names what `current` is, so a device knows whether its copy is.
    id: u64,
    next: u64,
    conflicts: Vec<(u64, Vec<u8>, String)>,
    /// Versions a device kept beside the file as files of their own.
    kept: usize,
}

impl Server {
    fn next(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

/// A device's copy of the file and what it has heard of the server.
struct Copy {
    bytes: Vec<u8>,
    /// The server's file this copy was, before any local commit.
    base: u64,
    dirty: bool,
    /// The conflict versions delivered to this device.
    versions: Vec<(u64, Vec<u8>, String)>,
    online: bool,
}

struct Device {
    name: String,
    copy: Copy,
    replica: Replica,
}

/// A device's copy of the file as its replica publishes to it.
struct Local<'a> {
    copy: &'a mut Copy,
    server: &'a mut Server,
}

fn refused() -> CommitError {
    CommitError {
        state: CommitState::NotCommitted,
        error: io::Error::from(io::ErrorKind::WouldBlock),
    }
}

impl Remote for Local<'_> {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.copy.bytes.clone())
    }

    fn stamp(&mut self) -> io::Result<Stamp> {
        Stamp::of(&self.copy.bytes).map_err(io::Error::other)
    }

    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        transaction
            .apply(&mut self.copy.bytes)
            .map_err(|_| refused())?;
        self.copy.dirty = true;
        Ok(())
    }

    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        match Stamp::of(&self.copy.bytes) {
            Ok(stamp) if stamp == *base => Ok(()),
            _ => Err(refused()),
        }
    }

    fn versions(&mut self) -> io::Result<Vec<Version>> {
        Ok(self
            .copy
            .versions
            .iter()
            .map(|(id, _, device)| Version {
                id: id.to_string(),
                device: Some(device.clone()),
            })
            .collect())
    }

    fn version(&mut self, id: &str) -> io::Result<Vec<u8>> {
        self.copy
            .versions
            .iter()
            .find(|(held, ..)| held.to_string() == id)
            .map(|(_, bytes, _)| bytes.clone())
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
    }

    fn retire(&mut self, id: &str, keep: bool) -> io::Result<()> {
        self.copy
            .versions
            .retain(|(held, ..)| held.to_string() != id);
        let before = self.server.conflicts.len();
        self.server
            .conflicts
            .retain(|(held, ..)| held.to_string() != id);
        if keep && before != self.server.conflicts.len() {
            self.server.kept += 1;
        }
        Ok(())
    }
}

/// A small xorshift generator; schedules replay from their seed.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn format() -> Format {
    Format {
        font: Some("Calibri".to_owned()),
        font_size: Some(11.0),
        language: Some(0x409),
        ..Default::default()
    }
}

fn paragraph(text: &str) -> PageParagraph {
    PageParagraph {
        id: new_id().unwrap(),
        parent: None,
        level: 1,
        style: None,
        format: Default::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Paragraph::new(text.into(), format()),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    }
}

/// `page` with a body outline holding one paragraph of `text`.
fn with_body(page: &Page, text: &str) -> Page {
    let mut page = page.clone();
    let outline = Outline {
        id: new_id().unwrap(),
        title: false,
        min_width: None,
        layout: Layout {
            x: Some(36.0),
            y: Some(86.0),
            ..Default::default()
        },
        indents: Vec::new(),
        paragraphs: vec![paragraph(text)],
        unsupported: Vec::new(),
    };
    page.objects.push(PageObject::Outline(outline));
    page
}

/// A section of three pages, each with a body paragraph.
fn fixture() -> Vec<u8> {
    let source = onestore::create_empty_section("cloud.one", None).unwrap();
    let arena = onestore::Arena::default();
    let mut section = Section::open(&arena, source).unwrap();
    for n in 0..3 {
        let creation = PageCreation::new(None, Some(&format!("Page {n}")), "Fixture").unwrap();
        let space = creation.space();
        edit(
            &mut section,
            "Fixture",
            vec![Op::Section(SectionOp::Create(creation))],
        );
        let page = section.page(space).unwrap();
        let body = with_body(&page, &format!("Body {n} with words"));
        let ops = lower_page(&page, &body)
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space, op })
            .collect();
        edit(&mut section, "Fixture", ops);
    }
    section.seal().unwrap();
    section.image()
}

fn edit(section: &mut Section<'_>, author: &str, ops: Vec<Op>) {
    section
        .apply(
            author,
            &Edit {
                at: 133_000_000_000_000_000,
                ops,
            },
        )
        .unwrap();
}

/// Every body text of `page`: its identity and characters.
fn bodies(page: &Page) -> Vec<(ExGuid, String)> {
    page.objects
        .iter()
        .filter_map(|object| match object {
            PageObject::Outline(outline) => Some(outline),
            _ => None,
        })
        .flat_map(|outline| &outline.paragraphs)
        .filter_map(|paragraph| match &paragraph.content {
            ParagraphContent::Text(text) => Some((text.id, text.text.text().to_owned())),
            _ => None,
        })
        .collect()
}

/// What a schedule typed, and where.
#[derive(Default)]
struct Typed {
    /// Each typed string with the page it went into.
    strings: Vec<(String, ExGuid)>,
    /// Pages some device deleted, whose strings may be gone.
    deleted: BTreeSet<ExGuid>,
}

impl Device {
    fn remote<'a>(&'a mut self, server: &'a mut Server) -> (Local<'a>, &'a Replica) {
        (
            Local {
                copy: &mut self.copy,
                server,
            },
            &self.replica,
        )
    }

    /// Commits what the replica holds and merges what it sees, as the sync thread does.
    fn sync(&mut self, server: &mut Server) {
        let (mut remote, replica) = self.remote(server);
        for _ in 0..8 {
            match replica.sync_once(&mut remote) {
                Ok(synced)
                    if matches!(
                        synced.edit,
                        Some((_, notebook::EditStatus::Published { .. }))
                    ) => {}
                Ok(_) => return,
                Err(notebook::Error::Remote(error)) if error.state == CommitState::NotCommitted => {
                }
                Err(error) => panic!("{}: {error}", self.name),
            }
        }
    }

    /// Sends a changed copy to the server; one made from a file the server has moved past
    /// becomes the file or a conflict version, either way round.
    fn upload(&mut self, server: &mut Server, random: &mut Random) {
        if !self.copy.online || !self.copy.dirty {
            return;
        }
        self.copy.dirty = false;
        if self.copy.base == server.id {
            server.current = self.copy.bytes.clone();
            server.id = server.next();
            self.copy.base = server.id;
            return;
        }
        let id = server.next();
        if random.below(2) == 0 {
            // This copy becomes the file; the server's goes beside it.
            let lost = std::mem::replace(&mut server.current, self.copy.bytes.clone());
            server.conflicts.push((id, lost, "The server".into()));
            server.id = server.next();
            self.copy.base = server.id;
        } else {
            server
                .conflicts
                .push((id, self.copy.bytes.clone(), self.name.clone()));
            self.copy.bytes = server.current.clone();
            self.copy.base = server.id;
        }
    }

    /// Takes the server's file, or conflicts with it while this copy has changes.
    fn download(&mut self, server: &mut Server, random: &mut Random) {
        if !self.copy.online || self.copy.base == server.id {
            return;
        }
        if self.copy.dirty {
            return self.upload(server, random);
        }
        self.copy.bytes = server.current.clone();
        self.copy.base = server.id;
    }

    /// Learns which conflict versions the server keeps.
    fn deliver(&mut self, server: &Server) {
        if self.copy.online {
            self.copy.versions = server.conflicts.clone();
        }
    }

    /// Makes a random edit, as someone using the device would.
    fn edit(&mut self, random: &mut Random, typed: &mut Typed, serial: &mut u32) {
        let pages = self.replica.pages().unwrap();
        let (space, ..) = pages[random.below(pages.len())].clone();
        let page = self.replica.page(space).unwrap();
        *serial += 1;
        let token = format!("⟨{}-{serial}⟩", self.name);
        let at = crate_now();
        let ops = match random.below(20) {
            0..=12 => {
                let texts = bodies(&page);
                if texts.is_empty() {
                    return;
                }
                let (text, characters) = &texts[random.below(texts.len())];
                // Anywhere but inside another typed string, so each stays whole to count.
                let mut inside = false;
                let mut boundaries = Vec::new();
                for (at, character) in characters.char_indices() {
                    if !inside {
                        boundaries.push(at);
                    }
                    inside = (inside || character == '⟨') && character != '⟩';
                }
                boundaries.push(characters.len());
                let byte = boundaries[random.below(boundaries.len())];
                let offset = characters[..byte].encode_utf16().count() as u32;
                vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text: *text,
                        range: offset..offset,
                        with: token.clone(),
                    },
                }]
            }
            13..=15 => {
                let mut after = page.clone();
                let Some(outline) = after.objects.iter_mut().find_map(|object| match object {
                    PageObject::Outline(outline) => Some(outline),
                    _ => None,
                }) else {
                    return;
                };
                let top: Vec<usize> = (0..outline.paragraphs.len())
                    .filter(|at| outline.paragraphs[*at].parent.is_none())
                    .collect();
                let at = top[random.below(top.len())];
                outline.paragraphs.insert(at + 1, paragraph(&token));
                lower_page(&page, &after)
                    .unwrap()
                    .into_iter()
                    .map(|op| Op::Page { space, op })
                    .collect()
            }
            16 => {
                // First or last, as a series starts at the first page.
                let before = (random.below(2) == 0).then_some(pages[0].0);
                let creation = PageCreation::new(before, Some(&token), &self.name).unwrap();
                let space = creation.space();
                return self.apply(
                    at,
                    vec![Op::Section(SectionOp::Create(creation))],
                    typed,
                    (token, space),
                );
            }
            17 if pages.len() > 1 => {
                typed.deleted.insert(space);
                vec![Op::Section(SectionOp::Delete(vec![space]))]
            }
            _ => {
                let before = pages
                    .get(random.below(pages.len() + 1))
                    .map(|page| page.0)
                    .filter(|before| *before != space);
                let first = pages[0].0;
                // A page going first stays at the top level.
                let level = if before == Some(first) {
                    1
                } else {
                    1 + random.below(2) as u32
                };
                vec![Op::Section(SectionOp::Pages(vec![
                    PageEdit::move_to(space, before, level).unwrap(),
                ]))]
            }
        };
        self.apply(at, ops, typed, (token, space));
    }

    /// Applies an edit, noting the string it typed once the section takes it.
    fn apply(&self, at: u64, ops: Vec<Op>, typed: &mut Typed, token: (String, ExGuid)) {
        let text = matches!(
            ops.first(),
            Some(Op::Page { .. } | Op::Section(SectionOp::Create(_)))
        );
        match self.replica.apply(&self.name, Edit { at, ops }) {
            Ok(_) if text => typed.strings.push(token),
            Ok(_) => {}
            Err(notebook::Error::Rejected(error)) if !text => {
                let _ = error;
            }
            Err(error) => panic!("{}: {error}", self.name),
        }
    }
}

fn crate_now() -> u64 {
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    (unix.as_secs() + 11_644_473_600) * 10_000_000
}

/// Every text on the section's pages, and on its conflict pages.
fn texts_of(image: &[u8]) -> (String, String) {
    let arena = onestore::Arena::default();
    let mut section = Section::open(&arena, image.to_vec()).unwrap();
    let mut listed = String::new();
    for (space, title, _) in section.pages().unwrap() {
        listed += &title;
        listed += "\n";
        for (_, text) in bodies(&section.page(space).unwrap()) {
            listed += &text;
            listed += "\n";
        }
    }
    let mut conflicts = String::new();
    for (_, pages) in section.conflicts().unwrap() {
        for conflict in pages {
            let page = section.page(conflict.space).unwrap();
            conflicts += &page.title;
            for (_, text) in bodies(&page) {
                conflicts += &text;
                conflicts += "\n";
            }
        }
    }
    (listed, conflicts)
}

/// Runs a schedule of `events` among `count` devices from `seed`, then lets every device
/// come online until nothing changes, and checks where they ended.
fn run(seed: u64, count: usize, events: usize) {
    let source = fixture();
    let directory = tempfile::tempdir().unwrap();
    let mut server = Server {
        current: source.clone(),
        id: 0,
        next: 0,
        conflicts: Vec::new(),
        kept: 0,
    };
    let mut devices: Vec<Device> = (0..count)
        .map(|n| Device {
            name: format!("D{n}"),
            copy: Copy {
                bytes: source.clone(),
                base: 0,
                dirty: false,
                versions: Vec::new(),
                online: true,
            },
            replica: Replica::create(directory.path().join(format!("{n}.sqlite")), &source)
                .unwrap(),
        })
        .collect();
    let mut random = Random(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
    let mut typed = Typed::default();
    let mut serial = 0;
    let trace = std::env::var("CLOUD_TRACE").ok();
    for event in 0..events {
        let index = random.below(count);
        let device = &mut devices[index];
        let kind = random.below(12);
        match kind {
            0..=3 => device.edit(&mut random, &mut typed, &mut serial),
            4..=5 => device.sync(&mut server),
            6..=7 => device.upload(&mut server, &mut random),
            8 => device.download(&mut server, &mut random),
            9 => device.deliver(&server),
            _ => device.copy.online = !device.copy.online,
        }
        if let Some(token) = &trace {
            report(
                token,
                &format!("{event} D{index} {kind}"),
                &server,
                &devices,
            );
        }
    }
    // Everyone online until a whole round changes nothing.
    for device in &mut devices {
        device.copy.online = true;
    }
    let mut rounds = 0;
    loop {
        rounds += 1;
        assert!(rounds < 64, "seed {seed}: no quiescence");
        let before = (server.id, server.conflicts.len());
        for device in 0..devices.len() {
            devices[device].download(&mut server, &mut random);
            devices[device].deliver(&server);
            devices[device].sync(&mut server);
            devices[device].upload(&mut server, &mut random);
            if let Some(token) = &trace {
                report(
                    token,
                    &format!("round {rounds} D{device}"),
                    &server,
                    &devices,
                );
            }
        }
        let settled = server.conflicts.is_empty()
            && (server.id, server.conflicts.len()) == before
            && devices.iter().all(|device| {
                !device.copy.dirty
                    && device.copy.base == server.id
                    && device.replica.pending().unwrap().is_empty()
            });
        if settled {
            break;
        }
    }
    for device in &devices {
        assert_eq!(
            device.copy.bytes, server.current,
            "seed {seed}: {}",
            device.name
        );
    }
    assert_eq!(server.kept, 0, "seed {seed}: a version could not merge");
    let (listed, conflicts) = texts_of(&server.current);
    for (string, space) in &typed.strings {
        // A page deleted on one device while another typed into it comes back as a copy, one
        // for the version merged and one for the edits queued since, if both hold edits.
        if typed.deleted.contains(space) {
            continue;
        }
        let shown = listed.matches(string.as_str()).count();
        assert!(
            shown <= 1,
            "seed {seed}: {string} shows {shown} times:\n{listed}"
        );
        assert!(
            shown == 1 || conflicts.contains(string.as_str()),
            "seed {seed}: {string} is gone:\n{listed}\n--\n{conflicts}"
        );
    }
}

#[test]
fn devices_on_a_cloud_drive_converge_with_every_edit_once() {
    for seed in sweep::seeds(0..200, 60) {
        run(seed, 3, 60);
    }
}

#[test]
fn two_devices_merging_one_version_at_once_add_it_once() {
    let source = fixture();
    let directory = tempfile::tempdir().unwrap();
    let mut server = Server {
        current: source.clone(),
        id: 0,
        next: 0,
        conflicts: Vec::new(),
        kept: 0,
    };
    let mut devices: Vec<Device> = (0..3)
        .map(|n| Device {
            name: format!("D{n}"),
            copy: Copy {
                bytes: source.clone(),
                base: 0,
                dirty: false,
                versions: Vec::new(),
                online: true,
            },
            replica: Replica::create(directory.path().join(format!("{n}.sqlite")), &source)
                .unwrap(),
        })
        .collect();
    let space = devices[0].replica.pages().unwrap()[0].0;
    let type_at = |device: &Device, token: &str| {
        let page = device.replica.page(space).unwrap();
        let (text, _) = bodies(&page)[0].clone();
        device
            .replica
            .apply(
                &device.name,
                Edit {
                    at: crate_now(),
                    ops: vec![Op::Page {
                        space,
                        op: PageOp::Text {
                            text,
                            range: 0..0,
                            with: token.into(),
                        },
                    }],
                },
            )
            .unwrap();
    };
    // D0 and D1 edit the same page; D1's upload loses and becomes a conflict version.
    type_at(&devices[0], "⟨zero⟩");
    type_at(&devices[1], "⟨one⟩");
    let mut random = Random(1);
    for device in &mut devices[..2] {
        device.sync(&mut server);
    }
    devices[0].upload(&mut server, &mut random);
    devices[1].upload(&mut server, &mut random);
    assert_eq!(server.conflicts.len(), 1);
    // D0 and D2 both see the version and merge it before either hears the other did.
    for device in [0, 2] {
        devices[device].download(&mut server, &mut random);
        devices[device].deliver(&server);
    }
    for device in [0, 2] {
        devices[device].sync(&mut server);
    }
    assert!(server.conflicts.is_empty());
    devices[0].upload(&mut server, &mut random);
    // D2's merge lands on the file D0's already changed: a conflict version again.
    devices[2].upload(&mut server, &mut random);
    assert_eq!(server.conflicts.len(), 1);
    for _ in 0..8 {
        for device in &mut devices {
            device.download(&mut server, &mut random);
            device.deliver(&server);
            device.sync(&mut server);
            device.upload(&mut server, &mut random);
        }
    }
    assert!(server.conflicts.is_empty());
    let (listed, conflicts) = texts_of(&server.current);
    for token in ["⟨zero⟩", "⟨one⟩"] {
        let shown = listed.matches(token).count() + conflicts.matches(token).count();
        assert!(
            (1..=2).contains(&shown) && listed.matches(token).count() <= 1,
            "{token} shows {shown} times:\n{listed}\n--\n{conflicts}"
        );
    }
    let arena = onestore::Arena::default();
    let mut section = Section::open(&arena, server.current.clone()).unwrap();
    let conflict_pages: usize = section
        .conflicts()
        .unwrap()
        .iter()
        .map(|(_, pages)| pages.len())
        .sum();
    assert!(conflict_pages <= 1, "{conflict_pages} conflict pages");
}

/// A version of a OneNote 2010 file merges into it: an edit to another paragraph replays, a
/// page the version made comes along, and an edit both made at one place becomes a conflict
/// page. `ONESTORE_ICLOUD_EXPORT` names a folder to write the merged notebook to, for a cold
/// read in OneNote 2010.
#[test]
fn a_version_of_a_onenote_file_merges_into_it() {
    let notebook = format!(
        "{}/../../corpus/conflict-page/native/initial/notebook",
        env!("CARGO_MANIFEST_DIR")
    );
    let source = std::fs::read(format!("{notebook}/synthetic.one")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut server = Server {
        current: source.clone(),
        id: 0,
        next: 0,
        conflicts: Vec::new(),
        kept: 0,
    };
    let mut devices: Vec<Device> = ["Mac", "iPhone"]
        .into_iter()
        .enumerate()
        .map(|(n, name)| Device {
            name: name.into(),
            copy: Copy {
                bytes: source.clone(),
                base: 0,
                dirty: false,
                versions: Vec::new(),
                online: true,
            },
            replica: Replica::create(directory.path().join(format!("{n}.sqlite")), &source)
                .unwrap(),
        })
        .collect();
    let space = devices[0].replica.pages().unwrap()[0].0;
    let texts = bodies(&devices[0].replica.page(space).unwrap());
    let (first, second) = (texts[0].clone(), texts[1].clone());
    let typing = |text: ExGuid, at: u32, with: &str| Op::Page {
        space,
        op: PageOp::Text {
            text,
            range: at..at,
            with: with.into(),
        },
    };
    let apply = |device: &Device, ops: Vec<Op>| {
        device
            .replica
            .apply(
                &device.name,
                Edit {
                    at: crate_now(),
                    ops,
                },
            )
            .unwrap();
    };
    apply(&devices[0], vec![typing(first.0, 0, "Mac: ")]);
    let end = second.1.encode_utf16().count() as u32;
    apply(
        &devices[1],
        vec![
            typing(first.0, 0, "iPhone: "),
            typing(second.0, end, " (typed on the iPhone)"),
        ],
    );
    let creation = PageCreation::new(None, Some("From the iPhone"), "iPhone").unwrap();
    let made = creation.space();
    apply(&devices[1], vec![Op::Section(SectionOp::Create(creation))]);
    let page = devices[1].replica.page(made).unwrap();
    let body = with_body(&page, "Written offline on the iPhone.");
    apply(
        &devices[1],
        lower_page(&page, &body)
            .unwrap()
            .into_iter()
            .map(|op| Op::Page { space: made, op })
            .collect(),
    );
    let mut random = Random(3);
    for device in &mut devices {
        device.sync(&mut server);
        device.upload(&mut server, &mut random);
    }
    assert_eq!(server.conflicts.len(), 1);
    let version = server.conflicts[0].clone();
    for _ in 0..6 {
        for device in &mut devices {
            device.download(&mut server, &mut random);
            device.deliver(&server);
            device.sync(&mut server);
            device.upload(&mut server, &mut random);
        }
    }
    assert!(server.conflicts.is_empty());
    assert!(
        devices
            .iter()
            .all(|device| device.copy.bytes == server.current)
    );
    // The version once more, as a device that has not heard it was resolved sees it: merged
    // already, it adds nothing.
    devices[1].copy.versions = vec![version];
    devices[1].sync(&mut server);
    assert!(!devices[1].copy.dirty && devices[1].copy.versions.is_empty());
    let (listed, conflicts) = texts_of(&server.current);
    assert!(listed.contains("(typed on the iPhone)"), "{listed}");
    assert!(
        listed.contains("Written offline on the iPhone."),
        "{listed}"
    );
    let arena = onestore::Arena::default();
    let merged = Section::open(&arena, server.current.clone()).unwrap();
    let shown: String = bodies(&merged.page(space).unwrap())
        .into_iter()
        .map(|(_, text)| text + "\n")
        .collect();
    assert_eq!(
        shown.matches("Mac: ").count() + shown.matches("iPhone: ").count(),
        1,
        "{shown}"
    );
    assert!(
        conflicts.contains("Mac: ") || conflicts.contains("iPhone: "),
        "{conflicts}"
    );
    if let Some(output) = std::env::var_os("ONESTORE_ICLOUD_EXPORT") {
        let output = std::path::Path::new(&output);
        std::fs::create_dir_all(output).unwrap();
        std::fs::write(output.join("synthetic.one"), &server.current).unwrap();
        std::fs::copy(
            format!("{notebook}/Open Notebook.onetoc2"),
            output.join("Open Notebook.onetoc2"),
        )
        .unwrap();
    }
}

/// Where `token` stands after `label`, for tracing a schedule (`CLOUD_TRACE`).
fn report(token: &str, label: &str, server: &Server, devices: &[Device]) {
    let has = |image: &[u8]| {
        let (listed, conflicts) = texts_of(image);
        format!(
            "{}{}",
            listed.matches(token).count(),
            conflicts.matches(token).count()
        )
    };
    let mut line = format!("{label}: server {}", has(&server.current));
    for (id, bytes, _) in &server.conflicts {
        line += &format!(" v{id}:{}", has(bytes));
    }
    for device in devices {
        let working: String = device
            .replica
            .pages()
            .unwrap()
            .iter()
            .map(|(space, title, _)| {
                title.clone()
                    + &bodies(&device.replica.page(*space).unwrap())
                        .into_iter()
                        .map(|(_, text)| text)
                        .collect::<String>()
            })
            .collect();
        line += &format!(
            " | {} copy {} base {} dirty {} replica {} pending {}",
            device.name,
            has(&device.copy.bytes),
            device.copy.base,
            u8::from(device.copy.dirty),
            working.matches(token).count(),
            device.replica.pending().unwrap().len()
        );
    }
    eprintln!("{line}");
}
