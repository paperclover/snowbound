use notebook::{EditStatus, Remote, Replica};
use onestore::{
    Arena, CommitError, ExGuid, PageCreation, Section, Stamp, Transaction,
    op::{Edit, Op, PageOp, SectionOp},
    page::PageObject,
};
use std::{io, path::PathBuf, time::Instant};

struct FileRemote(PathBuf);

impl Remote for FileRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        onestore::read_file(&self.0)
    }
    fn stamp(&mut self) -> io::Result<Stamp> {
        Stamp::of(&self.read()?).map_err(io::Error::other)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        transaction.commit_file(&self.0)
    }
    fn confirm(&mut self, base: &Stamp) -> Result<(), CommitError> {
        onestore::confirm_file(&self.0, base)
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    assert_eq!(args.len(), 2, "queue_scale NEW_DIRECTORY EDIT_COUNT");
    let directory = PathBuf::from(&args[0]);
    let count: usize = args[1].to_str().unwrap().parse()?;
    assert!(count >= 2);
    std::fs::create_dir(&directory)?;
    let source = onestore::create_section("queue.one", "First", "Fixture")?;
    let creation = PageCreation::new(None, Some("Second"), "Fixture")?;
    let (source, pages) = {
        let arena = Arena::default();
        let mut section = Section::open(&arena, source)?;
        let ops = vec![Op::Section(SectionOp::Create(creation))];
        section.apply("Fixture", &Edit { at: 133_000_000_000_000_000, ops })?;
        section.seal()?;
        let pages: Vec<ExGuid> = section.pages()?.into_iter().map(|(space, ..)| space).collect();
        (section.image(), pages)
    };
    assert_eq!(pages.len(), 2);
    let path = directory.join("cache.sqlite");
    let mut remote = FileRemote(directory.join("queue.one"));
    std::fs::write(&remote.0, &source)?;
    let cache = Replica::create(&path, &source)?;
    let mut ids = Vec::new();
    let mut expected = [String::new(), String::new()];
    let start = Instant::now();
    for n in 0..count {
        let edit_start = Instant::now();
        let slot = n % 2;
        let space = pages[slot];
        let page = cache.page(space)?;
        let text = page
            .objects
            .iter()
            .find_map(|object| match object {
                PageObject::Title(title) => title
                    .outlines
                    .iter()
                    .flat_map(|outline| &outline.paragraphs)
                    .find_map(|p| p.text().filter(|text| text.date_field.is_none())),
                _ => None,
            })
            .unwrap();
        expected[slot] = format!("Edit {n} 🦀 e\u{301}");
        let end = u32::try_from(text.text.text().encode_utf16().count())?;
        let op = PageOp::Text {
            text: text.id,
            range: 0..end,
            with: expected[slot].clone(),
        };
        let id = cache.apply(
            "Fixture",
            Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Page { space, op }],
            },
        )?;
        assert!(ids.last().is_none_or(|previous| *previous < id));
        ids.push(id);
        println!(
            "{}",
            serde_json::json!({"phase":"ack","n":n,"id":id,
            "ms":edit_start.elapsed().as_secs_f64()*1000.0})
        );
    }
    println!(
        "{}",
        serde_json::json!({"phase":"queued","count":count,
        "seconds":start.elapsed().as_secs_f64(),"cache_bytes":std::fs::metadata(&path)?.len()})
    );
    drop(cache);
    let start = Instant::now();
    let cache = Replica::open(&path)?;
    assert_eq!(
        cache
            .pending()?
            .iter()
            .map(|edit| edit.id)
            .collect::<Vec<_>>(),
        ids
    );
    verify(&cache, &pages, &expected)?;
    println!(
        "{}",
        serde_json::json!({"phase":"reopen","seconds":start.elapsed().as_secs_f64()})
    );
    let start = Instant::now();
    cache.export_recovery(directory.join("recovery.sqlite"))?;
    println!(
        "{}",
        serde_json::json!({"phase":"export","seconds":start.elapsed().as_secs_f64()})
    );
    let start = Instant::now();
    // The queue publishes as one batch.
    let step = Instant::now();
    assert!(matches!(
        cache.sync_once(&mut remote)?.edit,
        Some((actual, EditStatus::Published { .. })) if Some(&actual) == ids.last()
    ));
    println!(
        "{}",
        serde_json::json!({"phase":"publish","count":ids.len(),"ms":step.elapsed().as_secs_f64()*1000.0})
    );
    assert!(cache.pending()?.is_empty());
    let published = onestore::read_file(&remote.0)?;
    verify(&cache, &pages, &expected)?;
    let arena = Arena::default();
    let section = Section::open(&arena, published.clone())?;
    for space in &pages {
        assert_eq!(section.page(*space)?, cache.page(*space)?);
    }
    drop(cache);
    let cache = Replica::open(&path)?;
    for id in &ids {
        assert!(matches!(
            cache.status(*id)?,
            Some(EditStatus::Published { .. })
        ));
    }
    let archive = notebook::Recovery::open(directory.join("recovery.sqlite"))?;
    assert_eq!(
        archive
            .pending()?
            .iter()
            .map(|edit| edit.id)
            .collect::<Vec<_>>(),
        ids
    );
    let arena = Arena::default();
    let section = Section::open(&arena, archive.snapshot()?)?;
    for (slot, space) in pages.iter().enumerate() {
        assert_eq!(section.page(*space)?.title, expected[slot]);
    }
    println!(
        "{}",
        serde_json::json!({"phase":"complete","count":count,
        "publish_seconds":start.elapsed().as_secs_f64(),"cache_bytes":std::fs::metadata(&path)?.len(),
        "remote_bytes":published.len()})
    );
    Ok(())
}

fn verify(
    cache: &Replica,
    pages: &[ExGuid],
    expected: &[String; 2],
) -> Result<(), Box<dyn std::error::Error>> {
    for (slot, space) in pages.iter().enumerate() {
        assert_eq!(cache.page(*space)?.title, expected[slot]);
    }
    Ok(())
}
