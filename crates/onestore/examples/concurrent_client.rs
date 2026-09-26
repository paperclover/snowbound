#[path = "support/concurrent.rs"]
mod concurrent;
#[path = "../src/flush.rs"]
mod flush;
#[path = "support/typing.rs"]
mod typing;

use std::{env, fs, io::Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() == 2 && args[0] == "init" {
        let bytes =
            onestore::create_section("synthetic.one", "Concurrent edits:", "Concurrency test")?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&args[1])?;
        file.write_all(&bytes)?;
        flush::flush(&file)?;
        return Ok(());
    }
    concurrent::run(
        &args,
        |path| onestore::read_file(path),
        |path, source, space, object, range, replacement| {
            let edit = typing::text(space, object, range, replacement);
            match typing::sealed(source, "Concurrent client", &edit) {
                Ok(Some(transaction)) => transaction.commit_file(path),
                Ok(None) => Ok(()),
                Err(error) => Err(onestore::CommitError {
                    state: onestore::CommitState::NotCommitted,
                    error: std::io::Error::new(std::io::ErrorKind::InvalidData, error),
                }),
            }
        },
    )
}
