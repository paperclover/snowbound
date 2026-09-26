//! Publishes typed keystrokes to a Samba lab one at a time and reports each publication's
//! wall time: `smb_publish_probe ADDRESS KEYSTROKES [CONTROL]`. With `CONTROL`, the
//! `tools/smb-proxy.py` control file, each publication is marked in the proxy's trace.

use notebook::{
    EditStatus, Replica, SmbRemote,
    smb::{Client, Credentials},
};
use onestore::op::{Edit, Op, PageOp};
use std::{
    env, fs,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().skip(1).collect();
    let [address, count, control @ ..] = &args[..] else {
        return Err("smb_publish_probe ADDRESS KEYSTROKES [CONTROL]".into());
    };
    let count: u32 = count.parse()?;
    let mark = |phase: String| -> std::io::Result<()> {
        if let [control] = control {
            fs::write(
                format!("{control}.tmp"),
                format!("{{\"phase\":\"{phase}\"}}"),
            )?;
            fs::rename(format!("{control}.tmp"), control)?;
            // The proxy polls its control file every 50 ms.
            std::thread::sleep(Duration::from_millis(120));
        }
        Ok(())
    };
    let connect = || {
        Client::connect(
            address,
            "agent",
            Credentials::default(),
            Duration::from_secs(5),
        )
    };
    let path = format!(
        "probe-{}.one",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
    );
    let source = onestore::create_section(&path, "Body", "Probe")?;
    connect()?.create(&path, &source)?;
    let directory = tempfile::tempdir()?;
    let replica = Replica::create(directory.path().join("cache.sqlite"), &source)?;
    let space = replica.pages()?[0].0;
    let text = replica
        .page(space)?
        .objects
        .iter()
        .find_map(|object| match object {
            onestore::page::PageObject::Outline(outline) => outline
                .paragraphs
                .iter()
                .find_map(|p| p.text().map(|t| t.id)),
            _ => None,
        })
        .ok_or("no text")?;
    let mut remote = SmbRemote::new(connect()?, path.clone(), 1 << 24);
    mark("idle".into())?;
    let mut idle = Vec::new();
    for _ in 0..count {
        let started = Instant::now();
        assert_eq!(replica.sync_once(&mut remote)?.edit, None);
        idle.push(started.elapsed());
    }
    let mut published = Vec::new();
    for n in 0..count {
        replica.apply(
            "Probe",
            Edit {
                at: 133_000_000_000_000_000,
                ops: vec![Op::Page {
                    space,
                    op: PageOp::Text {
                        text,
                        range: 4 + n..4 + n,
                        with: "x".into(),
                    },
                }],
            },
        )?;
        mark(format!("publish-{n}"))?;
        let started = Instant::now();
        let synced = replica.sync_once(&mut remote)?;
        published.push(started.elapsed());
        assert!(matches!(
            synced.edit,
            Some((_, EditStatus::Published { .. }))
        ));
    }
    mark("done".into())?;
    let quantiles = |mut times: Vec<Duration>| {
        times.sort();
        let at = |q: f64| times[((times.len() - 1) as f64 * q) as usize].as_secs_f64() * 1000.0;
        format!(
            "p50 {:.2} ms, p90 {:.2} ms, max {:.2} ms",
            at(0.5),
            at(0.9),
            at(1.0)
        )
    };
    println!("idle poll: {}", quantiles(idle));
    println!("publish:   {}", quantiles(published));
    connect()?.delete(&path)?;
    Ok(())
}
