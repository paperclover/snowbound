//! Writes a section's pages, or one titled page, as the PDF Print and Export as PDF make.
//! `print_pdf SECTION.one NEW.pdf [TITLE] [--a4]`

use canvas::{layout::TextEngine, print};
use onestore::{RevisionIndex, Store, document::Document, page::Page};
use std::{env, fs, path::Path};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let [section, destination, rest @ ..] = args.as_slice() else {
        return Err("Expected a section, a new PDF path and optionally a page title".into());
    };
    let a4 = rest.iter().any(|arg| arg == "--a4");
    let title = rest.iter().find(|arg| *arg != "--a4");
    let bytes = fs::read(section)?;
    let store = Store::parse(&bytes)?;
    let index = RevisionIndex::parse(&store)?;
    let document = Document::parse(&index)?;
    let mut spaces: Vec<_> = document
        .pages()?
        .into_iter()
        .map(|(space, _)| space)
        .collect();
    spaces.dedup();
    let pages = spaces
        .into_iter()
        .map(|space| Page::from_space(&document, space))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|page| title.is_none_or(|title| page.title == *title))
        .collect();
    let name = Path::new(section)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default();
    let paper = if a4 { print::A4 } else { print::LETTER };
    let pdf = print::pdf(pages, &mut TextEngine::default(), paper, name)?;
    fs::write(destination, pdf)?;
    Ok(())
}
