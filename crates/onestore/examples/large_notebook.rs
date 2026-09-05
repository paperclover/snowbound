use onestore::{Store, create_section, create_table_of_contents};
use std::{env, fs, path::PathBuf};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let destination = PathBuf::from(args.next().ok_or("Provide a new destination directory.")?);
    let count: usize = args.next().ok_or("Provide a section count.")?.parse()?;
    let length: usize = args
        .next()
        .ok_or("Provide a text length in Unicode scalars.")?
        .parse()?;
    if args.next().is_some() || !(1..=1024).contains(&count) || !(1..=1_000_000).contains(&length) {
        return Err("Use 1–1024 sections and 1–1000000 Unicode scalars per page.".into());
    }
    fs::create_dir(&destination)?;
    let notebook = destination.join("notebook");
    fs::create_dir(&notebook)?;
    let alphabet: Vec<_> = "Repeated text · café 東京 مرحبا 😀 e\u{301} <>&\" "
        .chars()
        .collect();
    let mut sections = Vec::new();
    let mut expected = Vec::new();
    for index in 0..count {
        let name = format!("Section {index:03}.one");
        let text: String = (0..length)
            .map(|offset| {
                if offset % 250 == 249 {
                    '\r'
                } else {
                    alphabet[(offset + index) % alphabet.len()]
                }
            })
            .collect();
        let bytes = create_section(&name, &text, "Fictitious scale fixture")?;
        let identity = Store::parse(&bytes)?.header.file_id;
        fs::write(notebook.join(&name), bytes)?;
        expected.push(serde_json::json!({"section": name, "text": text}));
        sections.push((name, identity));
    }
    let entries: Vec<_> = sections
        .iter()
        .rev()
        .map(|(name, id)| (name.as_str(), *id))
        .collect();
    fs::write(
        notebook.join("Open Notebook.onetoc2"),
        create_table_of_contents("Open Notebook.onetoc2", &entries)?,
    )?;
    serde_json::to_writer(
        fs::File::create(destination.join("expected.json"))?,
        &expected,
    )?;
    Ok(())
}
