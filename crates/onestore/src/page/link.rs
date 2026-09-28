//! Links between pages, paragraphs and sections of the same notebook, in the form OneNote
//! stores them: a `HYPERLINK` field code whose URL names the section by its file identity
//! and the page by its notebook-management identity, with the section path as `base-path`.

use crate::ExGuid;

/// What an internal link opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkTarget<'a> {
    Section,
    Page {
        identity: [u8; 16],
        title: &'a str,
    },
    /// A paragraph or other object on a page, named by its stored identity.
    Object {
        identity: [u8; 16],
        title: &'a str,
        object: ExGuid,
    },
}

/// The URL OneNote 2010 stores for a link into the section whose file identity is `section`
/// and whose file is at `base_path` (the path as the linking client sees it):
/// `onenote:#Title&section-id={…}&page-id={…}&end&base-path=…`.
pub fn internal_link(section: [u8; 16], base_path: &str, target: LinkTarget<'_>) -> String {
    let guid = |bytes: [u8; 16]| {
        ExGuid { guid: bytes, n: 0 }
            .to_string()
            .split(',')
            .next()
            .unwrap()
            .to_owned()
    };
    let mut url = String::from("onenote:#");
    match target {
        LinkTarget::Section => {}
        LinkTarget::Page { title, .. } | LinkTarget::Object { title, .. } => {
            url.push_str(&encoded(title));
            url.push('&');
        }
    }
    url.push_str(&format!("section-id={}", guid(section)));
    match target {
        LinkTarget::Section => url.push_str("&end"),
        LinkTarget::Page { identity, .. } => {
            url.push_str(&format!("&page-id={}&end", guid(identity)));
        }
        LinkTarget::Object {
            identity, object, ..
        } => {
            url.push_str(&format!(
                "&page-id={}&object-id={}&{}",
                guid(identity),
                guid(object.guid),
                object.n
            ));
        }
    }
    url.push_str("&base-path=");
    url.push_str(base_path);
    url
}

/// The identities a stored internal link names. OneNote resolves links by identity, so
/// the title and base path in the URL are hints only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InternalLink {
    pub section: [u8; 16],
    pub page: Option<[u8; 16]>,
    pub object: Option<ExGuid>,
}

/// The identities in an `onenote:` link, whatever path it names before the `#` (none as
/// stored, the section file's as Copy Link to Page gives it), or `None` for any other URL.
pub fn parse_internal_link(url: &str) -> Option<InternalLink> {
    let guid = |value: &str| {
        format!("{value},0")
            .parse::<ExGuid>()
            .ok()
            .map(|id| id.guid)
    };
    let (_, fragment) = url.strip_prefix("onenote:")?.split_once('#')?;
    let parts: Vec<&str> = fragment.split('&').collect();
    let value = |name: &str| {
        parts.iter().find_map(|part| {
            part.strip_prefix(name)
                .and_then(|rest| rest.strip_prefix('='))
        })
    };
    let section = guid(value("section-id")?)?;
    let page = match value("page-id") {
        Some(id) => Some(guid(id)?),
        None => None,
    };
    let object = match parts.iter().position(|part| part.starts_with("object-id=")) {
        Some(at) => Some(ExGuid {
            guid: guid(&parts[at]["object-id=".len()..])?,
            n: parts.get(at + 1)?.parse().ok()?,
        }),
        None => None,
    };
    Some(InternalLink {
        section,
        page,
        object,
    })
}

/// Percent-encodes a page title the way OneNote does in a link fragment.
fn encoded(title: &str) -> String {
    let mut out = String::new();
    for byte in title.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
