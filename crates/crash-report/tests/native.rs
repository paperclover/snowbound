#![cfg(unix)]

#[test]
fn crash_child() {
    let Some(path) = std::env::var_os("SNOWBOUND_TEST_CRASH") else {
        return;
    };
    crash_report::hook("test", "native-test".into(), "test system".into(), |_| {});
    crash_report::set_path(path.into()).unwrap();
    #[cfg(target_os = "linux")]
    {
        extern "C" fn fatal(signal: i32, info: *mut libc::siginfo_t, _: *mut libc::c_void) {
            unsafe {
                crash_report::record_signal(signal, info);
                libc::raise(signal);
            }
        }
        let mut action: libc::sigaction = unsafe { std::mem::zeroed() };
        action.sa_sigaction = fatal as *const () as libc::sighandler_t;
        action.sa_flags = libc::SA_SIGINFO | libc::SA_RESETHAND;
        unsafe {
            libc::sigaction(libc::SIGABRT, &action, std::ptr::null_mut());
        }
    }
    if std::env::var_os("SNOWBOUND_TEST_PANIC").is_some() {
        crash_report::conceal("AI.one");
        let _ = std::panic::catch_unwind(|| panic!("Cannot read section AI"));
    }
    std::process::abort();
}

#[test]
fn fatal_signals_leave_a_report_and_preserve_the_first_panic() {
    use std::{
        os::unix::process::ExitStatusExt,
        process::{Command, Stdio},
    };
    let folder = tempfile::tempdir().unwrap();
    for panic in [false, true] {
        let report = folder
            .path()
            .join(if panic { "panic.txt" } else { "native.txt" });
        let mut child = Command::new(std::env::current_exe().unwrap());
        child
            .args(["--exact", "crash_child", "--nocapture"])
            .env("SNOWBOUND_TEST_CRASH", &report)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if panic {
            child.env("SNOWBOUND_TEST_PANIC", "1");
        }
        assert_eq!(child.status().unwrap().signal(), Some(libc::SIGABRT));
        let saved = std::fs::read_to_string(report).unwrap();
        assert!(
            saved.starts_with("Snowbound test, native-test\nSystem: test system\n"),
            "{saved}"
        );
        if panic {
            assert!(
                saved.contains("Panic: Cannot read section <name>"),
                "{saved}"
            );
            assert!(!saved.contains("Native signal"), "{saved}");
        } else {
            assert!(saved.contains("Native signal:"), "{saved}");
            assert!(!saved.contains("Fault address:"), "{saved}");
        }
    }
}
