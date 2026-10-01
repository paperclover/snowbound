//! `open_probe SECTION | --paragraphs N`: times opening a section through a fresh and a
//! reopened cache up to its page list and first page, then how long page reads wait while
//! the sync thread rebases the queue onto a changed remote.

use notebook::{Remote, Replica, session::Section};
use onestore::{
    CommitError, ExGuid, Stamp, Transaction,
    document::{Format, Layout},
    op::{Edit, Op, PageOp, SectionOp},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::{
    io,
    time::{Duration, Instant},
};

/// A section holding one page of `count` paragraphs.
fn generated(count: usize) -> Result<Vec<u8>, Box<dyn std::error::Error>> {
    let paragraph = |i: usize| PageParagraph {
        id: new_id().unwrap(),
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Paragraph::new(format!("Paragraph {i} of the probe"), Format::default()),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    };
    let arena = onestore::Arena::default();
    let mut section =
        onestore::Section::open(&arena, onestore::create_section("probe.one", "", "Probe")?)?;
    let page = Page {
        title: "Probe".into(),
        identity: None,
        created: None,
        margin_origin: [0.0; 2],
        rtl: false,
        color: None,
        rule_lines: None,
        objects: vec![PageObject::Outline(Outline {
            id: new_id()?,
            title: false,
            min_width: None,
            layout: Layout {
                x: Some(36.0),
                y: Some(86.4),
                ..Layout::default()
            },
            indents: Vec::new(),
            paragraphs: (0..count).map(paragraph).collect(),
            unsupported: Vec::new(),
        })],
        definitions: Default::default(),
    };
    let creation = onestore::PageCreation::new(None, Some("Probe"), "Probe")?;
    section.apply(
        "Probe",
        &Edit {
            at: 133_000_000_000_000_000,
            ops: vec![Op::Section(SectionOp::Import { creation, page })],
        },
    )?;
    section.seal()?;
    Ok(section.image())
}

/// The first text of the page with the most outline paragraphs, and that page's space.
fn text(page: &Page) -> Option<ExGuid> {
    page.objects.iter().find_map(|object| match object {
        PageObject::Outline(outline) => outline
            .paragraphs
            .iter()
            .find_map(|p| p.text().map(|t| t.id)),
        _ => None,
    })
}

struct Memory(Vec<u8>);

impl onestore::CommitIo for Memory {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = offset as usize;
        let size = output.len().min(self.0.len().saturating_sub(offset));
        output[..size].copy_from_slice(&self.0[offset..offset + size]);
        Ok(size)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = offset as usize;
        self.0.resize(self.0.len().max(offset + bytes.len()), 0);
        self.0[offset..offset + bytes.len()].copy_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Remote for Memory {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        Ok(self.0.clone())
    }
    fn stamp(&mut self) -> io::Result<Stamp> {
        Stamp::of(&self.0).map_err(io::Error::other)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        transaction.commit(self)
    }
    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        onestore::confirm(self, base)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let source = match (args.next().ok_or("SECTION | --paragraphs N")?, args.next()) {
        (flag, Some(count)) if flag == "--paragraphs" => generated(count.parse()?)?,
        (path, _) => std::fs::read(path)?,
    };
    let directory = tempfile::tempdir()?;
    let file = directory.path().join("section.one");
    std::fs::write(&file, &source)?;
    let cache = directory.path().join("cache");
    println!("section {} bytes", source.len());
    for label in ["fresh cache", "reopened cache"] {
        let start = Instant::now();
        let section = Section::open(&file, &cache, || {})?;
        let opened = start.elapsed();
        let pages = section.pages()?;
        let listed = start.elapsed();
        let page = section.page(pages[0].0)?;
        let read = start.elapsed();
        println!(
            "{label}: open {opened:.2?}, page list ({} pages) at {listed:.2?}, first page ({} objects) at {read:.2?}",
            pages.len(),
            page.objects.len()
        );
        section.close()?;
    }

    // A local edit queued against a remote another writer changed elsewhere: the next
    // step rebases, while this thread keeps reading the page list and a page.
    let replica = std::sync::Arc::new(Replica::create(
        directory.path().join("rebase.sqlite"),
        &source,
    )?);
    let (space, ..) = replica
        .pages()?
        .into_iter()
        .max_by_key(|(space, ..)| {
            replica
                .page(*space)
                .map(|page| page.objects.len())
                .unwrap_or(0)
        })
        .ok_or("a page")?;
    let target = text(&replica.page(space)?).ok_or("a text")?;
    let typed = |with: &str| Edit {
        at: 133_000_000_000_000_000,
        ops: vec![Op::Page {
            space,
            op: PageOp::Text {
                text: target,
                range: 0..0,
                with: with.into(),
            },
        }],
    };
    replica.apply("Probe", typed("Local "))?;
    let remote = {
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, source.clone())?;
        let root = section.root();
        let other = section
            .pages()?
            .into_iter()
            .map(|(space, ..)| space)
            .find(|other| *other != space);
        match other.and_then(|other| Some((other, text(&section.page(other).ok()?)?))) {
            Some((other, text)) => section.apply(
                "Native",
                &Edit {
                    at: 133_000_000_000_000_000,
                    ops: vec![Op::Page {
                        space: other,
                        op: PageOp::Text {
                            text,
                            range: 0..0,
                            with: "Remote ".into(),
                        },
                    }],
                },
            )?,
            None => section.apply(
                "Native",
                &Edit {
                    at: 133_000_000_000_000_000,
                    ops: vec![Op::Section(SectionOp::Create(onestore::PageCreation::new(
                        None,
                        Some("Remote"),
                        "Native",
                    )?))],
                },
            )?,
        }
        let _ = root;
        section.seal()?;
        section.image()
    };
    let syncing = std::sync::Arc::clone(&replica);
    let sync = std::thread::spawn(move || {
        let start = Instant::now();
        let result = syncing.sync_once(&mut Memory(remote));
        (start.elapsed(), result.map(|synced| synced.edit.is_some()))
    });
    let (mut slowest, mut reads) = (Duration::ZERO, 0);
    while !sync.is_finished() {
        let start = Instant::now();
        replica.pages()?;
        replica.page(space)?;
        slowest = slowest.max(start.elapsed());
        reads += 1;
    }
    let (rebase, published) = sync.join().map_err(|_| "the sync step panicked")?;
    println!(
        "rebase and publish {rebase:.2?} (published: {:?}); {reads} page-list + page reads meanwhile, slowest {slowest:.2?}",
        published?
    );
    Ok(())
}
