#[path = "../src/flush.rs"]
mod flush;

use std::{env, fs::OpenOptions, io::Write, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("Usage: create_section OUTPUT TEXT AUTHOR".into());
    }
    let name = Path::new(&args[0])
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("Invalid section name")?;
    let bytes = onestore::create_section(name, &args[1], &args[2])?;
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&args[0])?;
    file.write_all(&bytes)?;
    flush::flush(&file)?;
    Ok(())
}
