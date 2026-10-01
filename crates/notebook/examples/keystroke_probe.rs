//! `keystroke_probe PARAGRAPHS KEYSTROKES`: types into the middle paragraph of a page
//! of PARAGRAPHS paragraphs and reports, per keystroke, the time, the bytes the cache
//! writes to disk and the bytes the section file sees: queued alone, then each one
//! published at once, then idle polls. The section file stays in memory so the
//! process's disk writes (macOS `proc_pid_rusage`) are the cache's.

use notebook::{Remote, Replica};
use onestore::{
    CommitError, CommitIo, ExGuid, Stamp, Transaction,
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

fn disk_written() -> u64 {
    // SAFETY: proc_pid_rusage fills the zeroed structure for this process.
    unsafe {
        let mut info: libc::rusage_info_v2 = std::mem::zeroed();
        libc::proc_pid_rusage(
            libc::getpid(),
            libc::RUSAGE_INFO_V2,
            &mut info as *mut _ as *mut _,
        );
        info.ri_diskio_byteswritten
    }
}

/// The section file in memory, counting what synchronization reads and writes, so the
/// process's disk writes are the cache's alone.
struct Memory {
    bytes: Vec<u8>,
    read: u64,
    written: u64,
}

impl CommitIo for Memory {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        let offset = offset as usize;
        let size = output.len().min(self.bytes.len().saturating_sub(offset));
        output[..size].copy_from_slice(&self.bytes[offset..offset + size]);
        self.read += size as u64;
        Ok(size)
    }
    fn write_at(&mut self, offset: u64, bytes: &[u8]) -> io::Result<usize> {
        let offset = offset as usize;
        self.bytes
            .resize(self.bytes.len().max(offset + bytes.len()), 0);
        self.bytes[offset..offset + bytes.len()].copy_from_slice(bytes);
        self.written += bytes.len() as u64;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Remote for Memory {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        self.read += self.bytes.len() as u64;
        Ok(self.bytes.clone())
    }
    fn stamp(&mut self) -> io::Result<Stamp> {
        self.read += 1024;
        Stamp::of(&self.bytes).map_err(io::Error::other)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        transaction.commit(self)
    }
    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        onestore::confirm(self, base)
    }
}

fn paragraph(i: usize) -> PageParagraph {
    let words = [
        "quick", "brown", "fox", "jumps", "over", "the", "lazy", "dog",
    ];
    let text: Vec<&str> = (0..1 + i % 17).map(|j| words[(i * 7 + j) % 8]).collect();
    PageParagraph {
        id: new_id().unwrap(),
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Paragraph::new(text.join(" "), Format::default()),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    }
}

fn percentiles(label: &str, mut times: Vec<Duration>, bytes: &[u64]) {
    times.sort();
    let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize];
    let mean = bytes.iter().sum::<u64>() as f64 / bytes.len() as f64;
    let mut sorted = bytes.to_vec();
    sorted.sort();
    println!(
        "{label}: time p50 {:.2?} p90 {:.2?} max {:.2?}; bytes mean {:.0} p50 {} max {}",
        at(0.5),
        at(0.9),
        at(1.0),
        mean,
        sorted[sorted.len() / 2],
        sorted[sorted.len() - 1]
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let count: usize = args.next().ok_or("PARAGRAPHS")?.parse()?;
    let keystrokes: usize = args.next().map_or(Ok(200), |n| n.parse())?;
    let directory = tempfile::tempdir()?;
    // The probe page, stored in the section file before the cache opens.
    let source = {
        let arena = onestore::Arena::default();
        let mut section =
            onestore::Section::open(&arena, onestore::create_section("probe.one", "", "Probe")?)?;
        let creation = onestore::PageCreation::new(None, Some("Probe"), "Probe")?;
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
        section.apply(
            "Probe",
            &Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Section(SectionOp::Import { creation, page })],
            },
        )?;
        section.seal()?;
        section.image()
    };
    println!("section {} bytes, {count} paragraphs", source.len());
    let cache = Replica::create(directory.path().join("cache.sqlite"), &source)?;
    let (space, _, _) = cache
        .pages()?
        .into_iter()
        .find(|(_, title, _)| title == "Probe")
        .ok_or("the probe page")?;
    let target: ExGuid = cache
        .page(space)?
        .objects
        .iter()
        .find_map(|object| match object {
            PageObject::Outline(outline) if outline.paragraphs.len() == count => {
                outline.paragraphs[count / 2].text().map(|text| text.id)
            }
            _ => None,
        })
        .ok_or("the probe paragraph")?;
    let mut remote = Memory {
        bytes: source.clone(),
        read: 0,
        written: 0,
    };
    let mut at = 0_u32;
    let mut keystroke = |cache: &Replica| -> Result<(), notebook::Error> {
        cache.apply(
            "Probe",
            Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text: target,
                        range: at..at,
                        with: "x".into(),
                    },
                }],
            },
        )?;
        at += 1;
        Ok(())
    };

    let (mut times, mut bytes) = (Vec::new(), Vec::new());
    for _ in 0..keystrokes {
        let (disk, start) = (disk_written(), Instant::now());
        keystroke(&cache)?;
        times.push(start.elapsed());
        bytes.push(disk_written() - disk);
    }
    percentiles("queued keystroke (apply + SQLite commit)", times, &bytes);

    let (disk, read, written, start) =
        (disk_written(), remote.read, remote.written, Instant::now());
    cache.sync_once(&mut remote)?;
    println!(
        "publish the {keystrokes} queued keystrokes as one batch: {:.2?}; cache {} B, file writes {} B, file reads {} B",
        start.elapsed(),
        disk_written() - disk,
        remote.written - written,
        remote.read - read
    );

    let (mut times, mut cache_bytes, mut file_bytes, mut reads) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for _ in 0..keystrokes {
        let (disk, read, written, start) =
            (disk_written(), remote.read, remote.written, Instant::now());
        keystroke(&cache)?;
        cache.sync_once(&mut remote)?;
        times.push(start.elapsed());
        file_bytes.push(remote.written - written);
        cache_bytes.push(disk_written() - disk);
        reads.push(remote.read - read);
    }
    percentiles(
        "keystroke published at once: cache",
        times.clone(),
        &cache_bytes,
    );
    percentiles(
        "keystroke published at once: file writes",
        times.clone(),
        &file_bytes,
    );
    percentiles("keystroke published at once: file reads", times, &reads);

    let (disk, read) = (disk_written(), remote.read);
    for _ in 0..100 {
        assert!(cache.sync_once(&mut remote)?.edit.is_none());
    }
    println!(
        "100 idle polls: cache {} B, file reads {} B",
        disk_written() - disk,
        remote.read - read
    );
    Ok(())
}
