mod support {
    pub mod view;
}
use support::view::view;

use notebook::{EditStatus, Remote, Replica};
use onestore::{CommitError, CommitIo, PreparedEdit};
use serde_json::json;
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{self, Write},
    os::unix::fs::FileExt,
    path::Path,
};

fn phase(name: &str) {
    println!("{}", json!({"event":"phase", "name":name}));
    io::stdout().flush().unwrap();
    if env::var("ONESTORE_RECOVERY_PAUSE").ok().as_deref() == Some(name) {
        let mut line = String::new();
        assert!(
            io::stdin().read_line(&mut line).unwrap() > 0,
            "Controller closed a paused operation"
        );
    }
}

struct Disk {
    file: File,
    writes: usize,
    flushes: usize,
}

impl CommitIo for Disk {
    fn read_at(&mut self, offset: u64, output: &mut [u8]) -> io::Result<usize> {
        self.file.read_at(output, offset)
    }
    fn write_at(&mut self, offset: u64, data: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        println!(
            "{}",
            json!({"event":"write", "number":self.writes, "offset":offset, "bytes":data.len()})
        );
        phase(&format!("write-{}-before", self.writes));
        let result = self.file.write_at(data, offset);
        phase(&format!("write-{}-after", self.writes));
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        self.flushes += 1;
        phase(&format!("flush-{}-before", self.flushes));
        let result = self.file.sync_all();
        phase(&format!("flush-{}-after", self.flushes));
        result
    }
}

impl Remote for Disk {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        phase("read-before");
        let result = onestore::read_snapshot(
            |offset, output| self.file.read_at(output, offset),
            256 * 1024 * 1024,
        )
        .and_then(|snapshot| snapshot.ok_or_else(|| io::ErrorKind::WouldBlock.into()));
        phase("read-after");
        result
    }
    fn publish(&mut self, edit: &PreparedEdit<'_>) -> Result<(), CommitError> {
        phase("publish-before");
        let result = edit.commit(self);
        phase("publish-after");
        result
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        phase("confirm-before");
        let result = onestore::confirm_snapshot(self, snapshot);
        phase("confirm-after");
        result
    }
}

fn report(cache: &Replica, root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let local = view(&cache.snapshot()?)?;
    let remote = view(&fs::read(root.join("remote.one"))?)?;
    let (status, revision) = match cache.status(1)? {
        Some(EditStatus::Pending) => ("pending", None),
        Some(EditStatus::AwaitingConfirmation { revision }) => {
            ("uncertain", Some(revision.to_string()))
        }
        Some(EditStatus::Published { revision }) => ("published", Some(revision.to_string())),
        Some(EditStatus::Conflict(_)) => ("conflict", None),
        None => ("missing", None),
    };
    println!(
        "{}",
        json!({"event":"state", "status":status, "revision":revision,
        "local_text":local.text, "remote_text":remote.text, "remote_revision":remote.revision.to_string(),
        "pending":cache.pending()?.iter().map(|pending|match &pending.operation {
            notebook::Operation::Page(intent) => match intent.text_change() {
                Some((_, before, range, replacement)) => json!({"id":pending.id,"before":before,"replacement":replacement,"range":[range.start,range.end]}),
                None => json!({"id":pending.id,"operation":"page"}),
            },
            operation => json!({"id":pending.id,"operation":operation}),
        }).collect::<Vec<_>>() })
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let mode = args
        .first()
        .ok_or("Expected init|inspect|sync DIRECTORY [SOURCE]")?;
    let root = Path::new(args.get(1).ok_or("Missing owned directory")?);
    if mode == "init" && args.len() == 3 {
        let source = fs::read(&args[2])?;
        let target = view(&source)?;
        fs::create_dir(root)?;
        let mut remote = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(root.join("remote.one"))?;
        remote.write_all(&source)?;
        remote.sync_all()?;
        drop(remote);
        let cache = Replica::create(root.join("cache.sqlite"), &source)?;
        let at = u32::try_from(target.text.encode_utf16().count())?;
        let mut page = {
            let store = onestore::Store::parse(&source)?;
            let index = onestore::RevisionIndex::parse(&store)?;
            let document = onestore::document::Document::parse(&index)?;
            onestore::page::Page::from_space(&document, target.space)?
        };
        let paragraph = page
            .objects
            .iter_mut()
            .find_map(|object| match object {
                onestore::page::PageObject::Outline(outline) => outline
                    .paragraphs
                    .iter_mut()
                    .find(|p| p.text().is_some_and(|t| t.id == target.object)),
                _ => None,
            })
            .ok_or("Target paragraph is not on its page")?
            .text_mut()
            .unwrap();
        let format = paragraph.text.format_at(at)?.clone();
        paragraph.text.apply(onestore::page::text::Edit {
            range: at..at,
            replacement: onestore::page::Paragraph::new(" [offline-recovery]".into(), format),
        })?;
        assert_eq!(
            cache.save(&source, target.space, &page, "Fixture")?,
            Some(1)
        );
        phase("local-after");
        report(&cache, root)?;
    } else if args.len() == 2 && ["inspect", "sync"].contains(&mode.as_str()) {
        let cache = Replica::open(root.join("cache.sqlite"))?;
        if mode == "sync" {
            let mut disk = Disk {
                file: OpenOptions::new()
                    .read(true)
                    .write(true)
                    .open(root.join("remote.one"))?,
                writes: 0,
                flushes: 0,
            };
            phase("sync-before");
            let result = cache.sync_once(&mut disk);
            phase("sync-after");
            result?;
        }
        report(&cache, root)?;
    } else {
        return Err("Expected init|inspect|sync DIRECTORY [SOURCE]".into());
    }
    Ok(())
}
