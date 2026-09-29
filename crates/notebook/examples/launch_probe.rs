//! Temporary: measures a notebook's launch (catalog and background first pass).
//! `generate DIR N` | `local DIR CACHE` | `upload ADDRESS SHARE DIR ROOT` |
//! `smb ADDRESS SHARE ROOT CACHE [SECONDS]`

use notebook::{
    discover::{Entry, Limits, Local, Source, discover},
    session::{Background, Notebook},
};
use onestore::{
    document::{Format, Layout},
    op::{Edit, Op, SectionOp},
    page::{
        Outline, Page, PageObject, PageParagraph, Paragraph, ParagraphContent, TextObject,
        text::new_id,
    },
};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Counting<S>(S, u64, u64);

impl<S: Source> Source for Counting<S> {
    fn entries(&mut self, path: &str, limit: usize) -> io::Result<Vec<Entry>> {
        self.2 += 1;
        self.0.entries(path, limit)
    }
    fn read(&mut self, path: &str, limit: usize) -> io::Result<Vec<u8>> {
        let bytes = self.0.read(path, limit)?;
        self.1 += bytes.len() as u64;
        Ok(bytes)
    }
}

const LIMITS: Limits = Limits {
    entries: 100_000,
    bytes_per_file: 256 * 1024 * 1024,
    depth: 64,
};

fn page(count: usize) -> Result<Page> {
    let paragraph = |i: usize| PageParagraph {
        id: new_id().unwrap(),
        parent: None,
        level: 1,
        style: None,
        format: Format::default(),
        content: ParagraphContent::Text(TextObject {
            id: new_id().unwrap(),
            date_field: None,
            text: Paragraph::new(
                format!("Paragraph {i} of the launch probe, with some words."),
                Format::default(),
            ),
            tags: Vec::new(),
        }),
        lists: Vec::new(),
        tags: Vec::new(),
        media: Default::default(),
        collapsed: false,
    };
    Ok(Page {
        title: "Probe".into(),
        identity: None,
        created: None,
        margin_origin: [0.0; 2],
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
    })
}

fn generate(root: &Path, count: usize) -> Result<()> {
    let cache = tempfile::tempdir()?;
    let creation = onestore::PageCreation::new(None, Some("First"), "Probe")?;
    let mut notebook = Notebook::create(root, cache.path(), 0x00e4a88a, &creation)?;
    notebook.create_group("", "Group A")?;
    notebook.create_group("", "Group B")?;
    for n in 0..count {
        let folder = ["", "Group A", "Group B"][n % 3];
        let path = notebook.create_section(folder, &format!("Section {n:03}"), &creation)?;
        let file = root.join(&path);
        let arena = onestore::Arena::default();
        let mut section = onestore::Section::open(&arena, std::fs::read(&file)?)?;
        let creation = onestore::PageCreation::new(None, Some("Probe"), "Probe")?;
        section.apply(
            "Probe",
            &Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Section(SectionOp::Import {
                    creation,
                    page: page(1200)?,
                })],
            },
        )?;
        let transaction = section.seal()?.ok_or("nothing sealed")?;
        transaction.commit_file(&file)?;
    }
    Ok(())
}

fn size(root: &Path) -> u64 {
    std::fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size(&entry.path()),
            _ => entry.metadata().map_or(0, |metadata| metadata.len()),
        })
        .sum()
}

#[cfg(feature = "smb")]
fn client(address: &str, share: &str) -> Result<notebook::smb::Client> {
    Ok(notebook::smb::Client::connect(
        address,
        share,
        notebook::smb::Credentials {
            username: "",
            password: "",
            domain: "",
        },
        Duration::from_secs(10),
    )?)
}

#[cfg(feature = "smb")]
fn upload(client: &notebook::smb::Client, local: &Path, remote: &str) -> Result<()> {
    client.create_directory(remote)?;
    for entry in std::fs::read_dir(local)? {
        let entry = entry?;
        let name = entry.file_name().into_string().map_err(|_| "name")?;
        let target = format!("{remote}/{name}");
        if entry.file_type()?.is_dir() {
            upload(client, &entry.path(), &target)?;
        } else {
            client.create(&target, &std::fs::read(entry.path())?)?;
        }
    }
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["generate", root, count] => {
            generate(Path::new(root), count.parse()?)?;
            println!("{} bytes", size(Path::new(root)));
        }
        ["local", root, cache] => {
            let start = Instant::now();
            let notebook = Notebook::open(root, cache)?;
            let replicas = notebook.replicas();
            println!(
                "open: {:?}, {} sections",
                start.elapsed(),
                replicas.len()
            );
            let mut counting = Counting(Local::open(root)?, 0, 0);
            discover(&mut counting, LIMITS)?;
            println!("discovery reads {} bytes, {} listings", counting.1, counting.2);
        }
        #[cfg(feature = "smb")]
        ["upload", address, share, local, root] => {
            upload(&client(address, share)?, Path::new(local), root)?;
        }
        #[cfg(feature = "smb")]
        ["smb", address, share, root, cache, rest @ ..] => {
            let seconds: u64 = rest.first().map_or(Ok(0), |s| s.parse())?;
            let start = Instant::now();
            let connected = client(address, share)?;
            let notebook = Notebook::open_smb(std::sync::Arc::new(connected), root, cache)?;
            let replicas = notebook.replicas();
            println!(
                "open: {:?}, {} sections",
                start.elapsed(),
                replicas.len()
            );
            if seconds > 0 {
                let (address, share) = (address.to_string(), share.to_string());
                let background = Background::smb(
                    root,
                    256 * 1024 * 1024,
                    move || {
                        client(&address, &share)
                            .map_err(|error| io::Error::other(error.to_string()))
                    },
                    || {},
                )?;
                background.watch(replicas);
                let start = Instant::now();
                loop {
                    let status = background.status();
                    if status.iter().all(|(_, status)| status.synced.is_some()) {
                        println!("background first pass: {:?}", start.elapsed());
                        break;
                    }
                    if start.elapsed() > Duration::from_secs(seconds) {
                        println!("background still going after {seconds} s");
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                std::thread::sleep(Duration::from_secs(2));
            }
        }
        _ => return Err("usage".into()),
    }
    Ok(())
}
