//! Desktop crash-report consent and upload.

#[cfg(not(target_arch = "wasm32"))]
pub use crash_report::set_path;
pub use crash_report::{RENDERER, conceal, forget, hook, kept};

#[cfg(not(target_arch = "wasm32"))]
fn send(report: String) {
    // A development build may send to a site run locally.
    let address = std::env::var("SNOWBOUND_CRASH_SITE")
        .ok()
        .filter(|_| cfg!(debug_assertions))
        .unwrap_or_else(|| crash_report::ADDRESS.to_str().unwrap().to_owned());
    std::thread::spawn(move || {
        let sent = crate::update::agent(&address, std::time::Duration::from_secs(60))
            .post(&address)
            .header("Content-Type", "text/plain; charset=utf-8")
            .send(report.as_bytes());
        match sent {
            Ok(_) => forget(),
            Err(error) => eprintln!("Cannot send the crash report: {error}"),
        }
    });
}

#[cfg(target_arch = "wasm32")]
fn send(report: String) {
    crate::platform::send_crash(&report);
}

impl crate::State {
    /// Asks whether to send the report the last run left, if it left one.
    pub(crate) fn offer_crash_report(&self) {
        let Some(report) = kept() else {
            return;
        };
        let shown = report.clone();
        let reply = self.reply(move |state, pressed| {
            match pressed {
                0 => send(report),
                1 => state.show_crash_report(shown),
                _ => {}
            }
            if pressed > 1 {
                forget();
            }
            Ok(())
        });
        crate::platform::choose(
            "Snowbound quit unexpectedly last time.",
            "Send a report to help fix the problem. You can review the report before sending it.",
            &["Send Report", "Show Report", "Don't Send"],
            reply,
        );
    }

    fn show_crash_report(&self, report: String) {
        let shown = report.clone();
        let reply = self.reply(move |_, pressed| {
            if pressed == 0 {
                send(report);
            }
            if pressed != 0 {
                forget();
            }
            Ok(())
        });
        crate::platform::choose(
            "Crash report",
            &shown,
            &["Send Report", "Don't Send"],
            reply,
        );
    }
}
