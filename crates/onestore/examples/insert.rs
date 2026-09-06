#[path = "../src/flush.rs"]
mod flush;

use onestore::{Insertion, PreparedEdit};
use std::{fs, io::Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let usage = "Usage: insert paragraph INPUT OUTPUT|--in-place SPACE PARENT BEFORE|- TEXT AUTHOR\n       insert outline INPUT OUTPUT|--in-place SPACE PAGE X Y TEXT AUTHOR";
    let insertion = match args.first().map(String::as_str) {
        Some("paragraph") if args.len() == 8 => Insertion::paragraph(
            args[4].parse()?,
            if args[5] == "-" {
                None
            } else {
                Some(args[5].parse()?)
            },
            &args[6],
            &args[7],
        )?,
        Some("outline") if args.len() == 9 => Insertion::outline(
            args[4].parse()?,
            args[5].parse()?,
            args[6].parse()?,
            &args[7],
            &args[8],
        )?,
        _ => return Err(usage.into()),
    };
    let source = onestore::read_file(&args[1])?;
    let prepared = PreparedEdit::insert(&source, args[3].parse()?, &insertion)?;
    let mut record = serde_json::json!({"intent": insertion, "object": insertion.object(), "text_object": insertion.text_object()});
    if args[2] == "--in-place" {
        if let Err(error) = prepared.commit_file(&args[1]) {
            record["state"] = format!("{:?}", error.state).into();
            record["error"] = error.error.to_string().into();
            println!("{record}");
            std::process::exit(2);
        }
        record["state"] = "Committed".into();
    } else {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[2])?;
        file.write_all(prepared.as_bytes())?;
        flush::flush(&file)?;
        record["state"] = "Created".into();
    }
    println!("{record}");
    Ok(())
}
