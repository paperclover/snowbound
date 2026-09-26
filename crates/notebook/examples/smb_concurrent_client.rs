#[path = "../../onestore/examples/support/concurrent.rs"]
mod concurrent;
#[path = "support/edit.rs"]
mod edit;

use notebook::smb::{Client, Credentials};
use std::{env, time::Duration};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::connect(
        &env::var("ONESTORE_SMB_LAB")?,
        &env::var("ONESTORE_SMB_SHARE")?,
        Credentials::default(),
        Duration::from_secs(10),
    )?;
    let args: Vec<_> = env::args().skip(1).collect();
    concurrent::run(
        &args,
        |path| client.read(path, 256 * 1024 * 1024),
        |path, source, space, object, range, replacement| {
            client.commit_transaction(
                path,
                &edit::replaced(source, space, object, range, replacement)?,
            )
        },
    )
}
