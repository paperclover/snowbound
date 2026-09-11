use notebook::discover::{Limits, Local, discover};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let limits = Limits {
        entries: 100_000,
        bytes_per_file: 256 * 1024 * 1024,
        depth: 64,
    };
    let catalog = match args.as_slice() {
        [root] => discover(&mut Local::open(root)?, limits)?,
        #[cfg(feature = "smb")]
        [flag, address, share, root] if flag == "--smb" => {
            use notebook::smb::{Client, Credentials};
            let username = std::env::var("ONESTORE_SMB_USERNAME").unwrap_or_default();
            let password = std::env::var("ONESTORE_SMB_PASSWORD").unwrap_or_default();
            let domain = std::env::var("ONESTORE_SMB_DOMAIN").unwrap_or_default();
            let client = Client::connect(
                address.to_str().ok_or("Use a UTF-8 server address")?,
                share.to_str().ok_or("Use a UTF-8 share name")?,
                Credentials {
                    username: &username,
                    password: &password,
                    domain: &domain,
                },
                std::time::Duration::from_secs(5),
            )?;
            discover(
                &mut notebook::discover::Smb::new(
                    &client,
                    root.to_str().ok_or("Use a UTF-8 notebook path")?,
                )?,
                limits,
            )?
        }
        _ => return Err("Provide ROOT, or --smb ADDRESS SHARE ROOT with the smb feature".into()),
    };
    serde_json::to_writer_pretty(std::io::stdout().lock(), &catalog)?;
    Ok(())
}
