#[path = "../src/flush.rs"]
mod flush;

use std::{env, fs, io::Write, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: create_notebook DIRECTORY TEXT AUTHOR".into());
    }
    let section = onestore::create_section("synthetic.one", &args[1], &args[2])?;
    let identity = onestore::Store::parse(&section)?.header.file_id;
    let toc = onestore::create_table_of_contents(
        "Open Notebook.onetoc2",
        &[("synthetic.one", identity)],
    )?;
    fs::create_dir(&args[0])?;
    for (name, data) in [("synthetic.one", section), ("Open Notebook.onetoc2", toc)] {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(Path::new(&args[0]).join(name))?;
        file.write_all(&data)?;
        flush::flush(&file)?;
    }
    Ok(())
}
