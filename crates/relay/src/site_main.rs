//! `snowbound-site`: see `crates/relay/README.md`.

use relay::site::{self, Site};
use std::{net::TcpListener, path::PathBuf, process::ExitCode};

const USAGE: &str = "\
usage: snowbound-site [OPTION VALUE]...

Each option may also come from the environment as SNOWBOUND_SITE_<OPTION>, upper case with
underscores: SNOWBOUND_SITE_ROOT=/srv/snowbound/web. Flags win.

  --listen ADDRESS    where to listen (127.0.0.1:23593)
  --root FOLDER       the web build's folder (web)
  --web URL           where Open in Web goes, the code appended, as
                      https://snowbound.paperclover.net/?join= (none: no Open in Web)
  --crashes FOLDER    where crash reports are kept (crashes, beside the root)
  --trust-forwarded true|false
                      count senders by X-Forwarded-For, behind a proxy (false)
";

fn main() -> ExitCode {
    let mut listen = String::from("127.0.0.1:23593");
    let mut site = Site {
        root: PathBuf::from("web"),
        web: None,
        crashes: PathBuf::new(),
        trust_forwarded: false,
    };
    let mut crashes = None;
    let from_environment = std::env::vars().filter_map(|(key, value)| {
        let option = key.strip_prefix("SNOWBOUND_SITE_")?;
        Some((option.to_lowercase().replace('_', "-"), value))
    });
    let mut arguments = std::env::args().skip(1);
    let mut from_flags = Vec::new();
    while let Some(flag) = arguments.next() {
        if flag == "--help" {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        let (Some(option), Some(value)) = (flag.strip_prefix("--"), arguments.next()) else {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        };
        from_flags.push((option.to_owned(), value));
    }
    for (option, value) in from_environment.chain(from_flags) {
        match option.as_str() {
            "listen" => listen = value,
            "root" => site.root = PathBuf::from(value),
            "web" => site.web = Some(value).filter(|web| !web.is_empty()),
            "crashes" => crashes = Some(PathBuf::from(value)),
            "trust-forwarded" => match value.parse() {
                Ok(trust) => site.trust_forwarded = trust,
                Err(_) => {
                    eprintln!("--trust-forwarded {value}: true or false\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
            _ => {
                eprintln!("--{option}: no such option\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
    site.crashes = crashes.unwrap_or_else(|| site.root.with_file_name("crashes"));
    let listener = match TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Cannot listen on {listen}: {error}");
            return ExitCode::FAILURE;
        }
    };
    if let Ok(address) = listener.local_addr() {
        println!(
            "snowbound-site {} listening on {address}, serving {}",
            env!("CARGO_PKG_VERSION"),
            site.root.display()
        );
    }
    match site::serve(listener, site) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
