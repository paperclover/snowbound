//! `snowbound-relay`: see `crates/relay/README.md`.

use relay::server::{self, Config};
use std::{net::TcpListener, process::ExitCode, time::Duration};

const USAGE: &str = "\
usage: snowbound-relay [OPTION VALUE]...

Each option may also come from the environment as SNOWBOUND_RELAY_<OPTION>, upper case with
underscores: SNOWBOUND_RELAY_LISTEN=127.0.0.1:23592. Flags win.

  --listen ADDRESS                    where to listen (127.0.0.1:23592)
  --trust-forwarded true|false        count peers by X-Forwarded-For, behind a proxy (false)
  --max-connections N                 (256)
  --max-connections-per-address N     per IPv4 address or IPv6 /64 (16)
  --max-rooms N                       (128)
  --max-room-peers N                  (16)
  --max-message BYTES                 (262144)
  --queue BYTES                       waiting to go to one peer before it is dropped (1048576)
  --idle SECONDS                      silence before a connection is closed (600)
  --room-bytes-per-second BYTES       (4194304)
  --joins-per-minute N                per address (30)
  --room-joins-per-minute N           (30)
  --failures-per-minute N             wrong codes per address before a lockout (10)
  --burn-after N                      wrong codes before a code admits no one new (5)
  --pending SECONDS                   for a peer joining a code to meet its owner (20)
";

fn main() -> ExitCode {
    let mut listen = String::from("127.0.0.1:23592");
    let mut config = Config::default();
    let from_environment = std::env::vars().filter_map(|(key, value)| {
        let option = key.strip_prefix("SNOWBOUND_RELAY_")?;
        Some((option.to_lowercase().replace('_', "-"), value))
    });
    let mut arguments = std::env::args().skip(1);
    let mut from_flags = Vec::new();
    while let Some(flag) = arguments.next() {
        if flag == "--help" {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        let Some(option) = flag.strip_prefix("--") else {
            eprint!("{USAGE}");
            return ExitCode::from(2);
        };
        let (option, value) = match option.split_once('=') {
            Some((option, value)) => (option.to_owned(), value.to_owned()),
            None => match arguments.next() {
                Some(value) => (option.to_owned(), value),
                None => {
                    eprintln!("--{option} needs a value\n\n{USAGE}");
                    return ExitCode::from(2);
                }
            },
        };
        from_flags.push((option, value));
    }
    for (option, value) in from_environment.chain(from_flags) {
        let set = match option.as_str() {
            "listen" => {
                listen = value.clone();
                Ok(())
            }
            _ => set(&mut config, &option, &value),
        };
        if let Err(error) = set {
            eprintln!("--{option} {value}: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    }
    let listener = match TcpListener::bind(&listen) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("Cannot listen on {listen}: {error}");
            return ExitCode::FAILURE;
        }
    };
    match listener.local_addr() {
        Ok(address) => println!(
            "snowbound-relay {} listening on {address}",
            env!("CARGO_PKG_VERSION")
        ),
        Err(error) => eprintln!("{error}"),
    }
    match server::serve(listener, config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn set(config: &mut Config, option: &str, value: &str) -> Result<(), String> {
    let number = || value.parse::<u64>().map_err(|error| error.to_string());
    let count = || number().map(|number| number as usize);
    let small =
        || number().and_then(|number| u32::try_from(number).map_err(|error| error.to_string()));
    match option {
        "trust-forwarded" => {
            config.trust_forwarded = value.parse().map_err(|_| "true or false".to_owned())?
        }
        "max-connections" => config.max_connections = count()?,
        "max-connections-per-address" => config.max_connections_per_address = count()?,
        "max-rooms" => config.max_rooms = count()?,
        "max-room-peers" => config.max_room_peers = count()?,
        "max-message" => config.max_message = count()?,
        "queue" => config.queue = count()?,
        "idle" => config.idle = Duration::from_secs(number()?),
        "room-bytes-per-second" => config.room_bytes_per_second = number()?.max(1),
        "joins-per-minute" => config.joins_per_minute = small()?.max(1),
        "room-joins-per-minute" => config.room_joins_per_minute = small()?.max(1),
        "failures-per-minute" => config.failures_per_minute = small()?.max(1),
        "burn-after" => config.burn_after = small()?.max(1),
        "pending" => config.pending = Duration::from_secs(number()?),
        _ => return Err("no such option".into()),
    }
    Ok(())
}
