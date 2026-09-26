use notebook::{EditStatus, Remote, Replica};
use onestore::{
    CommitError, PageCreation, PreparedEdit, RevisionIndex, Store, Transaction,
    document::Document,
    page::{Page, PageObject, Paragraph, text::Edit},
};
use std::{io, path::PathBuf, time::Instant};

struct FileRemote(PathBuf);

impl Remote for FileRemote {
    fn read(&mut self) -> io::Result<Vec<u8>> {
        onestore::read_file(&self.0)
    }
    fn publish(&mut self, transaction: &Transaction) -> Result<(), CommitError> {
        transaction.commit_file(&self.0)
    }
    fn confirm(&mut self, snapshot: &[u8]) -> Result<(), CommitError> {
        onestore::confirm_file_snapshot(&self.0, snapshot)
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
    let source = PreparedEdit::create_page(&source, &creation)?
        .as_bytes()
        .to_vec();
    let pages = {
        let store = Store::parse(&source)?;
        let index = RevisionIndex::parse(&store)?;
        Document::parse(&index)?.pages()?
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
        let source = cache.snapshot()?;
        let store = Store::parse(&source)?;
        let index = RevisionIndex::parse(&store)?;
        let document = Document::parse(&index)?;
        let slot = n % 2;
        let space = pages[slot].0;
        let mut page = Page::from_space(&document, space)?;
        let text = page
            .objects
            .iter_mut()
            .find_map(|object| match object {
                PageObject::Outline(outline) => {
                    outline.paragraphs.iter_mut().find_map(|p| p.text_mut())
                }
                PageObject::Title(title) => title
                    .outlines
                    .iter_mut()
                    .flat_map(|outline| &mut outline.paragraphs)
                    .find_map(|p| p.text_mut().filter(|text| text.date_field.is_none())),
                _ => None,
            })
            .unwrap();
        expected[slot] = format!("Edit {n} 🦀 e\u{301}");
        let end = u32::try_from(text.text.text().encode_utf16().count())?;
        let format = text.text.format_at(0)?.clone();
        text.text.apply(Edit {
            range: 0..end,
            replacement: Paragraph::new(expected[slot].clone(), format),
        })?;
        let id = cache.save(&source, space, &page, "Fixture")?.unwrap();
        assert!(ids.last().is_none_or(|previous| *previous < id));
        ids.push(id);
        println!(
            "{}",
            serde_json::json!({"phase":"ack","n":n,"id":id,
            "ms":edit_start.elapsed().as_secs_f64()*1000.0,"working_bytes":source.len()})
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
    let source = cache.snapshot()?;
    verify(&source, &pages, &expected)?;
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
    for (n, id) in ids.iter().enumerate() {
        let step = Instant::now();
        assert!(
            matches!(cache.sync_once(&mut remote)?, Some((actual, EditStatus::Published { .. })) if actual == *id)
        );
        println!(
            "{}",
            serde_json::json!({"phase":"publish","n":n,"id":id,"ms":step.elapsed().as_secs_f64()*1000.0})
        );
    }
    assert!(cache.pending()?.is_empty());
    let published = cache.snapshot()?;
    assert_eq!(onestore::read_file(&remote.0)?, published);
    verify(&published, &pages, &expected)?;
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
    assert_eq!(archive.snapshot()?, source);
    println!(
        "{}",
        serde_json::json!({"phase":"complete","count":count,
        "publish_seconds":start.elapsed().as_secs_f64(),"cache_bytes":std::fs::metadata(&path)?.len(),
        "remote_bytes":published.len()})
    );
    Ok(())
}

fn verify(
    bytes: &[u8],
    pages: &[(onestore::ExGuid, onestore::ExGuid)],
    expected: &[String; 2],
) -> Result<(), Box<dyn std::error::Error>> {
    let store = Store::parse(bytes)?;
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    for (slot, (space, _)) in pages.iter().enumerate() {
        assert_eq!(Page::from_space(&document, *space)?.title, expected[slot]);
    }
    Ok(())
}
