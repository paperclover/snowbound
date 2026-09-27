//! Times the search index as the app builds and queries it: `search_probe NOTEBOOK_FOLDER
//! [QUERY]...` reads every section file, indexes its pages, runs each query over the whole
//! notebook, and finds each query on the largest page.

use canvas::{
    editor::CanvasEditor,
    layout::TextEngine,
    search::{Entry, Index, Query, page_matches},
};
use std::time::Instant;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let root = args.next().ok_or("Name a notebook folder")?;
    let queries: Vec<String> = args.collect();
    let mut files: Vec<_> = std::fs::read_dir(&root)?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "one"))
        .collect();
    files.sort();
    let mut index = Index::default();
    let (mut read, mut built, mut text) = (0.0, 0.0, 0);
    let mut largest = None;
    for file in &files {
        let start = Instant::now();
        let pages = notebook::session::stored_pages(&onestore::read_file(file)?)?;
        read += start.elapsed().as_secs_f64();
        let start = Instant::now();
        let key = file.display().to_string();
        for stored in pages {
            let entry = Entry::new(&key, stored.space, &stored.page, 0);
            let length = canvas::search::page_text(&stored.page).len();
            text += length;
            if largest.as_ref().is_none_or(|(size, _)| length > *size) {
                largest = Some((length, stored.page));
            }
            index.set(entry);
        }
        built += start.elapsed().as_secs_f64();
    }
    println!(
        "{} sections, {} pages, {} KB of text: read {:.0} ms, indexed {:.1} ms",
        files.len(),
        index.len(),
        text / 1024,
        read * 1e3,
        built * 1e3
    );
    let (_, page) = largest.ok_or("No pages")?;
    let start = Instant::now();
    let entry = Entry::new("again", onestore::ExGuid::default(), &page, 0);
    println!(
        "largest page, {:?}: indexed again in {:.2} ms",
        page.title,
        start.elapsed().as_secs_f64() * 1e3
    );
    drop(entry);
    let mut engine = TextEngine::default();
    let editor = CanvasEditor::from_page(page, &mut engine)?;
    for query in &queries {
        let parsed = Query::new(query);
        let start = Instant::now();
        let found = index.search(&parsed, |_| true);
        let searched = start.elapsed().as_secs_f64() * 1e3;
        let start = Instant::now();
        let matches = page_matches(&editor, &parsed);
        let found_on_page = start.elapsed().as_secs_f64() * 1e3;
        println!(
            "{query:?}: {} pages in {searched:.2} ms; {} matches on the largest page in {found_on_page:.2} ms",
            found.len(),
            matches.len()
        );
    }
    Ok(())
}
