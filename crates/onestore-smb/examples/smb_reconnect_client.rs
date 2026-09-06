#[path = "../../onestore/examples/support/concurrent.rs"]
mod concurrent;

use onestore::{
    CommitError, CommitState, ExGuid, RevisionIndex, Store,
    document::{Document, Kind},
};
use onestore_smb::{Client, Credentials};
use serde_json::json;
use std::{
    cell::RefCell,
    env, io, thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn paragraph(
    bytes: &[u8],
    space: ExGuid,
    object: ExGuid,
) -> Result<String, Box<dyn std::error::Error>> {
    let store = Store::parse(bytes)?;
    let index = RevisionIndex::parse(&store)?;
    index.validate_current()?;
    let document = Document::parse(&index)?;
    let space = &document.spaces[&space];
    let revision = &space.revisions[&space.contexts[&ExGuid::default()]];
    let Kind::RichText { text, .. } = &revision.nodes[&object].kind else {
        return Err("The append target is no longer text.".into());
    };
    Ok(text.clone())
}

// Only the owned append workload guarantees unique tokens that no writer removes.
fn retained(before: &str, token: &str, current: &str, state: CommitState) -> io::Result<bool> {
    let count = current.matches(token).count();
    if count == 0 && state != CommitState::Committed && current.starts_with(before) {
        return Ok(false);
    }
    if count == 1
        && state != CommitState::NotCommitted
        && current.starts_with(&format!("{before}{token}"))
    {
        return Ok(true);
    }
    Err(io::Error::other(
        "The append history contradicts the commit outcome.",
    ))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args
        .first()
        .is_none_or(|mode| !["read", "write"].contains(&mode.as_str()))
    {
        return Err("Reconnect testing requires the append-only workload.".into());
    }
    let address = env::var("ONESTORE_SMB_LAB")?;
    let share = env::var("ONESTORE_SMB_SHARE")?;
    let client = RefCell::new(Some(Client::connect(
        &address,
        &share,
        Credentials::default(),
        Duration::from_secs(5),
    )?));
    let read = |path: &str| {
        let mut session = client.borrow_mut();
        if session.is_none() {
            match Client::connect(
                &address,
                &share,
                Credentials::default(),
                Duration::from_secs(5),
            ) {
                Ok(fresh) => {
                    *session = Some(fresh);
                    println!(
                        "{}",
                        json!({"event":"transport_connected", "at_us":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros()})
                    );
                }
                Err(error) => {
                    println!(
                        "{}",
                        json!({"event":"transport_connect_error","error":error.to_string()})
                    );
                    return Err(io::ErrorKind::WouldBlock.into());
                }
            }
        }
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros();
        match session.as_ref().unwrap().read(path, 256 * 1024 * 1024) {
            Ok(bytes) => {
                if args.get(2).is_some_and(|actor| actor == "r0")
                    && let Some(folder) = env::var_os("ONESTORE_OFFLINE_FORMAT_REPLY_DIR")
                {
                    let folder = std::path::PathBuf::from(folder);
                    let captured = folder.join("offline-retired.one");
                    if !captured.exists()
                        && let Ok(marker) = std::fs::read(folder.join("offline-paused-w0.isolate"))
                        && let Ok(watched) = serde_json::from_slice::<serde_json::Value>(&marker)
                        && watched["after_us"]
                            .as_u64()
                            .is_some_and(|after| started > u128::from(after))
                    {
                        let sid: ExGuid = watched["space"]
                            .as_str()
                            .ok_or_else(|| io::Error::other("Missing watched space"))?
                            .parse()
                            .map_err(io::Error::other)?;
                        let rid: ExGuid = watched["revision"]
                            .as_str()
                            .ok_or_else(|| io::Error::other("Missing watched revision"))?
                            .parse()
                            .map_err(io::Error::other)?;
                        let store = Store::parse(&bytes).map_err(io::Error::other)?;
                        let index = RevisionIndex::parse(&store).map_err(io::Error::other)?;
                        if index
                            .spaces
                            .get(&sid)
                            .is_some_and(|space| !space.revisions.contains_key(&rid))
                        {
                            index.validate_current().map_err(io::Error::other)?;
                            std::fs::write(captured.with_extension("tmp"), &bytes)?;
                            std::fs::rename(captured.with_extension("tmp"), captured)?;
                            println!(
                                "{}",
                                json!({"event":"revision_retired", "space":sid.to_string(), "revision":rid.to_string(), "started_us":started, "finished_us":SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros()})
                            );
                        }
                    }
                }
                Ok(bytes)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::Other | io::ErrorKind::TimedOut | io::ErrorKind::NotConnected
                ) =>
            {
                println!(
                    "{}",
                    json!({"event":"transport_read_error","error":error.to_string()})
                );
                *session = None;
                Err(io::ErrorKind::WouldBlock.into())
            }
            result => result,
        }
    };
    concurrent::run(
        &args,
        read,
        |path, source, space, object, range, replacement| {
            let outcome = client.borrow().as_ref().unwrap().commit_text(
                path,
                source,
                space,
                object,
                range,
                replacement,
            );
            let Err(error) = outcome else {
                return Ok(());
            };
            if error.state == CommitState::NotCommitted
                && matches!(
                    error.error.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::ResourceBusy
                        | io::ErrorKind::PermissionDenied
                        | io::ErrorKind::NotFound
                )
            {
                return Err(error);
            }
            println!(
                "{}",
                json!({"event":"transport_commit_error","state":format!("{:?}",error.state),"token":replacement,"error":error.error.to_string()})
            );
            *client.borrow_mut() = None;
            let deadline = Instant::now() + Duration::from_secs(60);
            loop {
                if Instant::now() >= deadline {
                    return Err(error);
                }
                let current = match read(path) {
                    Ok(bytes) => bytes,
                    Err(retry) if retry.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    Err(_) => return Err(error),
                };
                let published = paragraph(source, space, object)
                    .and_then(|before| {
                        Ok(retained(
                            &before,
                            replacement,
                            &paragraph(&current, space, object)?,
                            error.state,
                        )?)
                    })
                    .map_err(|failure| CommitError {
                        state: error.state,
                        error: io::Error::other(failure.to_string()),
                    })?;
                if published {
                    let confirmation = client.borrow().as_ref().unwrap().commit_text(
                        path,
                        &current,
                        space,
                        object,
                        0..0,
                        "",
                    );
                    if let Err(failure) = confirmation
                        && failure.state != CommitState::Committed
                    {
                        println!(
                            "{}",
                            json!({"event":"transport_confirmation_error","state":format!("{:?}", failure.state),"error":failure.error.to_string()})
                        );
                        *client.borrow_mut() = None;
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }
                    println!(
                        "{}",
                        json!({"event":"transport_reconciled","token":replacement,"published":true,"flush_confirmed":true})
                    );
                    return Ok(());
                }
                println!(
                    "{}",
                    json!({"event":"transport_reconciled","token":replacement,"published":false})
                );
                return Err(CommitError {
                    state: CommitState::NotCommitted,
                    error: io::ErrorKind::ResourceBusy.into(),
                });
            }
        },
    )
}

#[test]
fn uncertain_append_requires_one_retained_token_and_its_predecessor() {
    for state in [CommitState::Unknown, CommitState::Committed] {
        assert!(retained("before", " [w0:0]", "before [w0:0] [w1:0]", state).unwrap());
    }
    for state in [CommitState::Unknown, CommitState::NotCommitted] {
        assert!(!retained("before", " [w0:0]", "before [w1:0]", state).unwrap());
    }
    for (current, state) in [
        ("before [w0:0] [w0:0]", CommitState::Unknown),
        ("changed [w0:0]", CommitState::Unknown),
        ("befor", CommitState::Unknown),
        ("before", CommitState::Committed),
        ("before [w0:0]", CommitState::NotCommitted),
    ] {
        assert!(retained("before", " [w0:0]", current, state).is_err());
    }
}
