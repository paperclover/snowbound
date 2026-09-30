//! Printing on Linux: the desktop's print dialog through the XDG print portal, which prints
//! the PDF itself; without a portal the PDF opens in the desktop's viewer to print from.

use std::{collections::HashMap, error::Error, time::Duration};
use winit::window::Window;
use zbus::zvariant::{Fd, OwnedValue, Value};

/// Countries whose paper is US Letter; everywhere else prints on A4.
const LETTER_COUNTRIES: [&str; 16] = [
    "US", "CA", "MX", "PR", "PH", "CL", "CO", "VE", "GT", "CR", "PA", "DO", "SV", "NI", "HN", "BZ",
];

/// The paper of the locale's country, as `LC_PAPER` names it.
pub fn paper() -> [f32; 2] {
    let locale = ["LC_ALL", "LC_PAPER", "LANG"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
        .unwrap_or_default();
    paper_of(&locale)
}

/// The paper of `locale`'s country, such as `en_GB.UTF-8`'s.
fn paper_of(locale: &str) -> [f32; 2] {
    let country = locale
        .split(['.', '@'])
        .next()
        .and_then(|language| language.split('_').nth(1))
        .unwrap_or("US");
    if LETTER_COUNTRIES.contains(&country) {
        canvas::print::LETTER
    } else {
        canvas::print::A4
    }
}

/// Asks the print portal for the print dialog, then prints `pdf` as it was set up; the
/// dialog belongs to the portal, so a thread of its own waits on it.
pub fn print(_window: &Window, pdf: &[u8], title: &str) -> Result<(), Box<dyn Error>> {
    let path = crate::print::spooled(pdf, title)?;
    let title = title.to_owned();
    std::thread::spawn(move || {
        if let Err(error) = portal(&path, &title) {
            eprintln!("The print portal failed, opening the PDF instead: {error}");
            crate::platform::reveal(&path);
        }
    });
    Ok(())
}

fn portal(path: &std::path::Path, title: &str) -> Result<(), Box<dyn Error>> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(5))
        .build()?;
    let portal = zbus::blocking::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Print",
    )?;
    // A request's path follows from the sender and the token, so its answer can be awaited
    // before it is asked for.
    let sender = connection
        .unique_name()
        .ok_or("No D-Bus name")?
        .trim_start_matches(':')
        .replace('.', "_");
    let request = |token: &str| -> Result<_, Box<dyn Error>> {
        let path = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
        let request = zbus::blocking::Proxy::new(
            &connection,
            "org.freedesktop.portal.Desktop",
            path,
            "org.freedesktop.portal.Request",
        )?;
        Ok(request.receive_signal("Response")?)
    };
    let token = format!("snowbound{}", std::process::id());
    let mut answers = request(&token)?;
    let options = HashMap::from([("handle_token", Value::from(token.as_str()))]);
    let empty: HashMap<&str, Value> = HashMap::new();
    let _: zbus::zvariant::OwnedObjectPath =
        portal.call("PreparePrint", &("", title, &empty, &empty, &options))?;
    let answer = answers.next().ok_or("The print dialog did not answer")?;
    let (response, results): (u32, HashMap<String, OwnedValue>) = answer.body().deserialize()?;
    // 1 is cancelled.
    if response != 0 {
        return Ok(());
    }
    let prepared = results
        .get("token")
        .and_then(|token| u32::try_from(token).ok())
        .ok_or("The print dialog gave no token")?;
    let file = std::fs::File::open(path)?;
    let options = HashMap::from([("token", Value::from(prepared))]);
    let _: zbus::zvariant::OwnedObjectPath =
        portal.call("Print", &("", title, Fd::from(&file), &options))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn countries_pick_their_paper() {
        assert_eq!(paper_of("en_GB.UTF-8"), canvas::print::A4);
        assert_eq!(paper_of("es_MX.UTF-8"), canvas::print::LETTER);
        assert_eq!(paper_of("C"), canvas::print::LETTER);
    }
}
