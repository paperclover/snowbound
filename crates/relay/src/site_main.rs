//! `snowbound-site`: see `crates/relay/README.md`.

use relay::site::{self, Site};
use std::{net::TcpListener, path::PathBuf, process::ExitCode};

const USAGE: &str = "\
usage: snowbound-site [OPTION VALUE]...

Each option may also come from the environment as SNOWBOUND_SITE_<OPTION>, upper case:
SNOWBOUND_SITE_ROOT=/srv/snowbound/web. Flags win.

  --listen ADDRESS    where to listen (127.0.0.1:23593)
  --root FOLDER       the web build's folder (web)
  --web URL           where Open in Web goes, the code appended, as
                      https://snowbound.paperclover.net/?join= (none: no Open in Web)
";

fn main() -> ExitCode {
    let mut listen = String::from("127.0.0.1:23593");
    let mut site = Site {
        root: PathBuf::from("web"),
        web: None,
    };
    let from_environment = std::env::vars().filter_map(|(key, value)| {
        Some((key.strip_prefix("SNOWBOUND_SITE_")?.to_lowercase(), value))
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
            _ => {
                eprintln!("--{option}: no such option\n\n{USAGE}");
                return ExitCode::from(2);
            }
        }
    }
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
