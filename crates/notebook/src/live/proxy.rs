//! The proxy a relay connection, and only a relay connection, goes through: `HTTPS_PROXY`
//! (`HTTP_PROXY` for `ws://`), then `ALL_PROXY`, each in lower case too, short of `NO_PROXY`;
//! else the system's: macOS's network settings, Windows's Internet settings, GNOME's. Only
//! HTTP proxies, reached by `CONNECT`, with a name and password where the URL has them; a
//! proxy auto-configuration script is not read.

use base64::Engine;
use std::sync::Mutex;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proxy {
    pub host: String,
    pub port: u16,
    /// The name and password `Proxy-Authorization` sends.
    pub credentials: Option<(String, String)>,
}

impl Proxy {
    /// `http://[name:password@]host[:port][/]`, or `host:port`; none for another scheme.
    pub fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        let rest = match url.split_once("://") {
            Some(("http" | "https", rest)) => rest,
            Some(_) => return None,
            None => url,
        };
        let rest = rest.split('/').next()?;
        let (credentials, authority) = match rest.rsplit_once('@') {
            Some((credentials, authority)) => {
                let (name, password) = credentials.split_once(':').unwrap_or((credentials, ""));
                (
                    Some((crate::live::decode(name), crate::live::decode(password))),
                    authority,
                )
            }
            None => (None, rest),
        };
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) if !port.contains(']') => (host, port.parse().ok()?),
            _ => (authority, 80),
        };
        let host = host.trim_start_matches('[').trim_end_matches(']');
        (!host.is_empty()).then(|| Self {
            host: host.to_owned(),
            port,
            credentials,
        })
    }

    /// The `Proxy-Authorization` header's value, where it has credentials.
    pub fn authorization(&self) -> Option<String> {
        let (name, password) = self.credentials.as_ref()?;
        let token = base64::engine::general_purpose::STANDARD.encode(format!("{name}:{password}"));
        Some(format!("Basic {token}"))
    }
}

impl std::fmt::Display for Proxy {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{}:{}", self.host, self.port)
    }
}

/// A proxy `use_proxy` set in place of what the environment and the system say.
static CHOSEN: Mutex<Option<Option<Proxy>>> = Mutex::new(None);

/// Uses `proxy`, or no proxy with `Some(None)`, for every relay connection from now on; `None`
/// goes back to the environment's and the system's.
pub fn use_proxy(proxy: Option<Option<Proxy>>) {
    *CHOSEN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = proxy;
}

/// The proxy a connection to `host` goes through, over TLS where `tls`.
pub fn for_host(host: &str, tls: bool) -> Option<Proxy> {
    if let Some(chosen) = CHOSEN.lock().unwrap_or_else(|p| p.into_inner()).clone() {
        return chosen;
    }
    let variable = |name: &str| {
        [name.to_owned(), name.to_lowercase()]
            .into_iter()
            .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
    };
    let named = [if tls { "HTTPS_PROXY" } else { "HTTP_PROXY" }, "ALL_PROXY"]
        .into_iter()
        .find_map(variable);
    match named {
        Some(url) => {
            let bypass = variable("NO_PROXY").unwrap_or_default();
            (!bypassed(host, bypass.split(','))).then(|| Proxy::parse(&url))?
        }
        None => system(host, tls),
    }
}

/// Whether `host` is one of `list`'s: a name, a suffix after a dot, or `*` for every host.
fn bypassed<'a>(host: &str, list: impl Iterator<Item = &'a str>) -> bool {
    let host = host.to_ascii_lowercase();
    list.map(|entry| {
        entry
            .trim()
            .trim_start_matches("*.")
            .trim_start_matches('.')
    })
    .filter(|entry| !entry.is_empty())
    .any(|entry| {
        let entry = entry.to_ascii_lowercase();
        entry == "*" || host == entry || host.ends_with(&format!(".{entry}"))
    })
}

/// What `scutil --proxy` says of the network settings in use.
#[cfg(target_os = "macos")]
fn system(host: &str, tls: bool) -> Option<Proxy> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .arg("--proxy")
        .output()
        .ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    let value = |key: &str| {
        text.lines().find_map(|line| {
            let (name, value) = line.split_once(" : ")?;
            (name.trim() == key).then(|| value.trim().to_owned())
        })
    };
    let kind = if tls { "HTTPS" } else { "HTTP" };
    if value(&format!("{kind}Enable")).as_deref() != Some("1") {
        return None;
    }
    let exceptions: Vec<String> = text
        .lines()
        .skip_while(|line| !line.contains("ExceptionsList"))
        .skip(1)
        .take_while(|line| !line.contains('}'))
        .filter_map(|line| Some(line.split_once(" : ")?.1.trim().to_owned()))
        .collect();
    if bypassed(host, exceptions.iter().map(String::as_str)) {
        return None;
    }
    Some(Proxy {
        host: value(&format!("{kind}Proxy"))?,
        port: value(&format!("{kind}Port"))?.parse().ok()?,
        credentials: None,
    })
}

/// What Windows's Internet settings say, as WinHTTP and Internet Explorer read them for this
/// user: `ProxyServer`, as `host:port` or `https=host:port;http=...`, where `ProxyEnable`.
#[cfg(windows)]
fn system(host: &str, tls: bool) -> Option<Proxy> {
    let key = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    if registry::dword(key, "ProxyEnable")? == 0 {
        return None;
    }
    let server = registry::string(key, "ProxyServer")?;
    let overrides = registry::string(key, "ProxyOverride").unwrap_or_default();
    if bypassed(
        host,
        overrides.split(';').filter(|entry| *entry != "<local>"),
    ) {
        return None;
    }
    let scheme = if tls { "https=" } else { "http=" };
    let server = match server.contains('=') {
        true => server
            .split(';')
            .find_map(|entry| entry.trim().strip_prefix(scheme))?,
        false => server.as_str(),
    };
    Proxy::parse(server)
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod registry {
    use windows_sys::Win32::System::Registry::{
        HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
    };

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain([0]).collect()
    }

    pub fn dword(key: &str, name: &str) -> Option<u32> {
        let (key, name) = (wide(key), wide(name));
        let mut value = 0u32;
        let mut size = 4u32;
        // SAFETY: the key and name are NUL-terminated and outlive the call; the value is four
        // bytes, as `size` says.
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_DWORD,
                std::ptr::null_mut(),
                (&mut value as *mut u32).cast(),
                &mut size,
            )
        };
        (result == 0).then_some(value)
    }

    pub fn string(key: &str, name: &str) -> Option<String> {
        let (key, name) = (wide(key), wide(name));
        let mut buffer = vec![0u16; 2048];
        let mut size = (buffer.len() * 2) as u32;
        // SAFETY: as in `dword`, with `size` the buffer's length in bytes.
        let result = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                std::ptr::null_mut(),
                buffer.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if result != 0 {
            return None;
        }
        let length = buffer
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(buffer.len());
        Some(String::from_utf16_lossy(&buffer[..length]))
    }
}

/// What GNOME's proxy settings say, where they are manual.
#[cfg(not(any(target_os = "macos", windows)))]
fn system(host: &str, tls: bool) -> Option<Proxy> {
    let get = |schema: &str, key: &str| {
        let output = std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .output()
            .ok()?;
        let text = String::from_utf8(output.stdout).ok()?;
        Some(text.trim().trim_matches('\'').to_owned())
    };
    if get("org.gnome.system.proxy", "mode")? != "manual" {
        return None;
    }
    let ignored = get("org.gnome.system.proxy", "ignore-hosts").unwrap_or_default();
    let ignored = ignored.trim_matches(['[', ']']).replace('\'', "");
    if bypassed(host, ignored.split(',')) {
        return None;
    }
    let schema = if tls {
        "org.gnome.system.proxy.https"
    } else {
        "org.gnome.system.proxy.http"
    };
    Some(Proxy {
        host: get(schema, "host").filter(|host| !host.is_empty())?,
        port: get(schema, "port")?
            .parse()
            .ok()
            .filter(|port| *port != 0)?,
        credentials: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxies_read_as_written() {
        assert_eq!(
            Proxy::parse("http://ada:p%40ss@proxy.example:3128/"),
            Some(Proxy {
                host: "proxy.example".into(),
                port: 3128,
                credentials: Some(("ada".into(), "p@ss".into())),
            })
        );
        assert_eq!(Proxy::parse("proxy:8080").unwrap().port, 8080);
        assert_eq!(Proxy::parse("http://[::1]:8080").unwrap().host, "::1");
        assert!(Proxy::parse("socks5://proxy:1080").is_none());
        let proxy = Proxy::parse("http://ada:secret@proxy:1").unwrap();
        assert_eq!(proxy.authorization().unwrap(), "Basic YWRhOnNlY3JldA==");
        let list = || ["localhost", ".internal", "*.corp.example"].into_iter();
        assert!(bypassed("localhost", list()) && bypassed("files.internal", list()));
        assert!(bypassed("a.corp.example", list()) && !bypassed("relay.example", list()));
        assert!(bypassed("anything", ["*"].into_iter()));
    }
}
