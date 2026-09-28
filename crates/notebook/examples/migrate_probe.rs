//! migrate_probe CACHE...: converts copies of schema-14 caches, reports what they hold
//! and, where nothing blocks the queue, publishes it to an in-memory copy of the remote.
use notebook::{Remote, Replica};
use onestore::{CommitError, CommitIo, Stamp, Transaction};
use std::io;

struct Memory(Vec<u8>);

impl CommitIo for Memory {
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

fn pages(replica: &Replica) -> Result<Vec<onestore::page::Page>, notebook::Error> {
    replica
        .pages()?
        .into_iter()
        .map(|(space, ..)| replica.page(space))
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    for path in std::env::args().skip(1) {
        let copy = std::env::temp_dir().join(format!(
            "migrate-probe-{}-{}",
            std::process::id(),
            std::path::Path::new(&path)
                .file_name()
                .unwrap()
                .to_string_lossy()
        ));
        std::fs::copy(&path, &copy)?;
        // A cache open or crashed in WAL mode keeps committed pages in its `-wal` file.
        let wal = format!("{path}-wal");
        if std::path::Path::new(&wal).exists() {
            let mut target = copy.as_os_str().to_owned();
            target.push("-wal");
            std::fs::copy(&wal, target)?;
        }
        let started = std::time::Instant::now();
        match Replica::open(&copy) {
            Ok(replica) => {
                let elapsed = started.elapsed();
                let pending = replica.pending()?;
                let summary = replica.recovery_summary()?;
                println!(
                    "{path}: converted in {:.1} ms; {} edits; {:?}; conflict pages {:?}",
                    elapsed.as_secs_f64() * 1e3,
                    pending.len(),
                    summary,
                    replica.conflicts()?
                );
                for edit in &pending {
                    println!(
                        "  {} {:?} {} ops",
                        edit.id,
                        replica.status(edit.id)?,
                        edit.edit.ops.len()
                    );
                }
                for (space, title, level) in replica.pages()? {
                    println!("  page {space} {level} {title:?}");
                }
                let blocked = replica.recovery_summary()?.uncertain_edits > 0;
                if !pending.is_empty() && !blocked {
                    let local = pages(&replica)?;
                    // The remote the queue was made on, as a recovery archive records it.
                    let archive = copy.with_extension("probe-recovery");
                    replica.export_recovery(&archive)?;
                    let mut remote = Memory(notebook::Recovery::open(&archive)?.remote_snapshot()?);
                    std::fs::remove_file(&archive)?;
                    let synced = replica.sync_once(&mut remote)?;
                    let published = {
                        let arena = onestore::Arena::default();
                        let mut section = onestore::Section::open(&arena, remote.0.clone())?;
                        section
                            .pages()?
                            .into_iter()
                            .map(|(space, ..)| section.page(space))
                            .collect::<Result<Vec<_>, _>>()?
                    };
                    println!(
                        "  published {:?}; remote pages equal the converted pages: {}",
                        synced.edit.map(|(id, _)| id),
                        published == local
                    );
                }
            }
            Err(error) => println!("{path}: {error}"),
        }
    }
    Ok(())
}
