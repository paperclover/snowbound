use super::*;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, process::Child};

#[derive(Clone)]
struct Snapshot {
    content: BTreeMap<ExGuid, String>,
    modified: BTreeMap<(ExGuid, ExGuid), u32>,
}

fn snapshot(bytes: &[u8]) -> Snapshot {
    let store = Store::parse(bytes).unwrap();
    assert!(store.checksum_mismatches.is_empty());
    let index = RevisionIndex::parse(&store).unwrap();
    index.validate_current().unwrap();
    Document::parse(&index).unwrap();
    let mut content = BTreeMap::new();
    let mut modified = BTreeMap::new();
    for (sid, space) in &index.spaces {
        let revision = index
            .resolve(*sid, space.labels[&(ExGuid::default(), 1)])
            .unwrap();
        let mut objects = BTreeMap::new();
        for (oid, object) in &revision.objects {
            if let Some(onestore::FileDataReference::Internal(guid)) =
                object.file_reference().unwrap()
            {
                store.file_data(guid).unwrap();
            }
            let data = match object.data {
                onestore::ObjectData::Properties(bytes) => {
                    let mut properties = onestore::PropertySets::parse(bytes).unwrap();
                    for property in &mut properties.sets[0] {
                        if property.id == 0x14001d7a {
                            let onestore::Value::Bytes(value) = property.value else {
                                panic!()
                            };
                            assert!(
                                modified
                                    .insert(
                                        (*sid, *oid),
                                        u32::from_le_bytes(value.try_into().unwrap())
                                    )
                                    .is_none()
                            );
                            property.value = onestore::Value::Bytes(&[0; 4]);
                        }
                    }
                    format!("{properties:?}")
                }
                other => format!("{other:?}"),
            };
            objects.insert(
                *oid,
                (
                    object.jcid,
                    object.reference_count,
                    data,
                    format!("{:?}", object.references().unwrap()),
                ),
            );
        }
        content.insert(*sid, format!("{:?} {objects:?}", revision.roots));
    }
    Snapshot { content, modified }
}

fn stamp() -> u32 {
    (SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 315532800)
        .try_into()
        .unwrap()
}

fn published(
    actual: &Snapshot,
    old: &Snapshot,
    new: &Snapshot,
    time: std::ops::RangeInclusive<u32>,
) -> bool {
    if actual.content == old.content {
        assert_eq!(
            actual.modified, old.modified,
            "Unpublished modification time changed"
        );
        return false;
    }
    assert_eq!(
        actual.content, new.content,
        "Partial or unexpected publication"
    );
    assert!(actual.modified.keys().eq(new.modified.keys()));
    for (key, value) in &actual.modified {
        if old.modified.get(key) != new.modified.get(key) {
            assert!(
                time.contains(value),
                "Modification time outside the commit interval"
            );
        } else {
            assert_eq!(
                *value, new.modified[key],
                "Unrelated modification time changed"
            );
        }
    }
    true
}

#[test]
fn publication_oracle_bounds_changed_timestamps_and_preserves_others() {
    let id = ExGuid::default();
    let other = ExGuid { n: 1, ..id };
    let old = Snapshot {
        content: BTreeMap::from([(id, "old".into())]),
        modified: BTreeMap::from([((id, id), 10), ((id, other), 7)]),
    };
    let new = Snapshot {
        content: BTreeMap::from([(id, "new".into())]),
        modified: BTreeMap::from([((id, id), 20), ((id, other), 7)]),
    };
    let mut actual = new.clone();
    actual.modified.insert((id, id), 30);
    assert!(published(&actual, &old, &new, 25..=35));
    assert!(!published(&old, &old, &new, 25..=35));
    for (key, value) in [((id, id), 24), ((id, id), 36), ((id, other), 30)] {
        let mut changed = actual.clone();
        changed.modified.insert(key, value);
        assert!(std::panic::catch_unwind(|| published(&changed, &old, &new, 25..=35)).is_err());
    }
    actual.content = old.content.clone();
    assert!(std::panic::catch_unwind(|| published(&actual, &old, &new, 25..=35)).is_err());
}

struct Proxy(Child);
impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn records(output: &Path) -> Vec<Value> {
    let data = fs::read_to_string(output.join("proxy.jsonl")).unwrap();
    data.rsplit_once('\n')
        .map_or("", |(complete, _)| complete)
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn configure(output: &Path, state: Value) -> usize {
    fs::write(output.join("control.tmp"), state.to_string()).unwrap();
    fs::rename(output.join("control.tmp"), output.join("control.json")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = records(output);
        if let Some(index) = events
            .iter()
            .rposition(|event| event.get("control") == Some(&state))
        {
            return index + 1;
        }
        assert!(
            Instant::now() < deadline,
            "proxy did not acknowledge its control state"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn proxy(output: &Path) -> (Proxy, String) {
    let address = std::env::var("ONESTORE_SMB_LAB").unwrap();
    let (host, port) = address.rsplit_once(':').unwrap();
    let mut proxy = Proxy(
        std::process::Command::new("python3")
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/smb-proxy.py"))
            .arg(output.join("control.json"))
            .args(["--port", "0", "--server", host, "--server-port", port])
            .stdout(fs::File::create(output.join("proxy.jsonl")).unwrap())
            .stderr(fs::File::create(output.join("proxy.stderr")).unwrap())
            .spawn()
            .unwrap(),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    let port = loop {
        if let Some(port) = records(output)
            .iter()
            .find_map(|event| event["listening"].as_u64())
        {
            break port;
        }
        assert!(proxy.0.try_wait().unwrap().is_none());
        assert!(Instant::now() < deadline, "proxy did not start");
        std::thread::sleep(Duration::from_millis(5));
    };
    (proxy, format!("127.0.0.1:{port}"))
}

#[test]
#[ignore = "requires an owned Samba share and a new ONESTORE_SMB_EVIDENCE directory"]
fn live_asset_loss() {
    let output = std::path::PathBuf::from(std::env::var("ONESTORE_SMB_EVIDENCE").unwrap());
    fs::create_dir(&output).unwrap();
    let (_proxy, address) = proxy(&output);
    let observer = client();
    let bytes: Vec<_> = (0..1048577).map(|index| (index % 251) as u8).collect();
    let path = format!(
        "asset-loss-{}.onebin",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    create(&observer, &path, &bytes);
    for (command, occurrence) in [(8, 1), (8, 9), (8, 17), (8, 18), (6, 1)] {
        for direction in ["request", "response"] {
            let reader = Client::connect(
                &address,
                "agent",
                Credentials::default(),
                Duration::from_secs(5),
            )
            .unwrap();
            // Response occurrences count only replies with the selected status.
            let begin = configure(
                &output,
                json!({"cut":command,"occurrence":if command == 8 && occurrence == 18 && direction == "response" { 1 } else { occurrence },
                    "direction":direction,"status":if command == 8 && occurrence == 18 { "0xc0000011" } else { "0x0" }}),
            );
            assert!(
                reader.read_asset(&path, bytes.len()).is_err(),
                "{command}/{occurrence}/{direction}"
            );
            assert_eq!(
                reader.read_asset(&path, bytes.len()).unwrap_err().kind(),
                io::ErrorKind::NotConnected
            );
            assert_eq!(
                records(&output)[begin..]
                    .iter()
                    .filter(|row| row.get("cut").is_some())
                    .count(),
                1
            );
            configure(
                &output,
                json!({"phase":format!("{command}-{occurrence}-{direction}-reconnected")}),
            );
            let reconnected = Client::connect(
                &address,
                "agent",
                Credentials::default(),
                Duration::from_secs(5),
            )
            .unwrap();
            assert_eq!(reconnected.read_asset(&path, bytes.len()).unwrap(), bytes);
        }
    }
    assert_eq!(observer.read_asset(&path, bytes.len()).unwrap(), bytes);
}

#[test]
#[ignore = "requires an owned Samba share and a new ONESTORE_SMB_EVIDENCE directory"]
fn live_message_loss() {
    let output = std::path::PathBuf::from(std::env::var("ONESTORE_SMB_EVIDENCE").unwrap());
    fs::create_dir(&output).unwrap();
    fs::create_dir(output.join("interrupted")).unwrap();
    fs::create_dir(output.join("recovered")).unwrap();
    fs::create_dir(output.join("source")).unwrap();
    let (_proxy, proxied) = proxy(&output);
    let observer = client();
    let prefix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let fixtures = [
        (
            "chunked",
            onestore::create_section("fault.one", "Before café 🦀", "Fault test").unwrap(),
        ),
        (
            "carry",
            fs::read(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("../../corpus/append/round-01/tx-255/notebook/synthetic.one"),
            )
            .unwrap(),
        ),
    ];
    let mut results = Vec::new();
    for (fixture, source) in fixtures {
        fs::write(
            output.join("source").join(format!("{fixture}.one")),
            &source,
        )
        .unwrap();
        let (_, _, before) = text(&source);
        let after = if fixture == "chunked" {
            "After café 🦀 ".repeat(6000)
        } else {
            "After café 🦀".to_owned()
        };
        let suffix = " [reconnected]";
        fs::write(
            output.join(format!("{fixture}-intent.json")),
            serde_json::to_vec(&json!({"before":before,"after":after,"suffix":suffix})).unwrap(),
        )
        .unwrap();
        let old = snapshot(&source);
        let deadline = Instant::now() + Duration::from_secs(2);
        while old.modified.values().any(|value| *value >= stamp()) {
            assert!(
                Instant::now() < deadline,
                "source timestamps did not precede the test"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        let everything = 0..before.encode_utf16().count() as u32;
        let new = snapshot(&applied(
            &source,
            &replaced(&source, everything.clone(), &after),
        ));
        let baseline_path = format!("fault-{prefix}-{fixture}-baseline.one");
        create(&observer, &baseline_path, &source);
        configure(&output, json!({"phase":format!("{fixture}-setup")}));
        let baseline = Client::connect(
            &proxied,
            "agent",
            Credentials::default(),
            Duration::from_secs(5),
        )
        .unwrap();
        let start = configure(&output, json!({"phase":format!("{fixture}-baseline")}));
        let began = stamp();
        baseline
            .commit_transaction(
                &baseline_path,
                &replaced(&source, everything.clone(), &after),
            )
            .unwrap();
        let events = records(&output);
        let mut occurrences = BTreeMap::new();
        let mut cuts = Vec::new();
        for event in &events[start..] {
            let Some(direction) = event["direction"].as_str() else {
                continue;
            };
            if event["status"] == "0x103" {
                continue;
            }
            let command = event["command"].as_u64().unwrap();
            let status = event["status"].as_str().map(str::to_owned);
            let count = occurrences
                .entry((direction.to_owned(), command, status.clone()))
                .or_insert(0);
            *count += 1;
            cuts.push((direction.to_owned(), command, status, *count));
        }
        assert!(
            cuts.iter()
                .any(|(direction, command, _, count)| direction == "response"
                    && *command == 7
                    && *count == 3)
        );
        assert!(published(
            &snapshot(&observer.read(&baseline_path, 1 << 20).unwrap()),
            &old,
            &new,
            began..=stamp()
        ));
        drop(baseline);
        for (case, (direction, command, status, occurrence)) in cuts.iter().enumerate() {
            let name = format!("{fixture}-{case:03}");
            let path = format!("fault-{prefix}-{name}.one");
            create(&observer, &path, &source);
            configure(&output, json!({"phase":format!("{name}-setup")}));
            let interrupted = Client::connect(
                &proxied,
                "agent",
                Credentials::default(),
                Duration::from_secs(5),
            )
            .unwrap();
            let start = configure(
                &output,
                json!({"phase":name, "direction":direction, "cut":command, "status":status, "occurrence":occurrence}),
            );
            let began = stamp();
            let transaction = replaced(&source, everything.clone(), &after);
            let error = interrupted
                .commit_transaction(&path, &transaction)
                .unwrap_err();
            assert_eq!(
                interrupted.read(&path, 1 << 20).unwrap_err().kind(),
                io::ErrorKind::NotConnected
            );
            let events = records(&output);
            assert_eq!(
                events[start..]
                    .iter()
                    .filter(|event| event.get("cut").is_some())
                    .count(),
                1
            );
            assert!(
                !events
                    .iter()
                    .any(|event| event.get("trace_error").is_some())
            );
            let fresh = client();
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                match fresh
                    .open(&path, true)
                    .and_then(|file| file.coordinate(&path, true, &[]))
                {
                    Ok(file) => {
                        file.close().unwrap();
                        break;
                    }
                    Err(error)
                        if error.kind() == io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(error) => panic!("{name}: retired session retained exclusion: {error}"),
                }
            }
            let saved = fresh.read(&path, 1 << 20).unwrap();
            fs::write(
                output.join("interrupted").join(format!("{name}.one")),
                &saved,
            )
            .unwrap();
            let visible = published(&snapshot(&saved), &old, &new, began..=stamp());
            match error.state {
                CommitState::NotCommitted => assert!(!visible, "{name}"),
                CommitState::Committed => assert!(visible, "{name}"),
                CommitState::Unknown => {}
            }
            if !visible {
                fresh
                    .commit_transaction(&path, &replaced(&saved, everything.clone(), &after))
                    .unwrap();
            }
            let saved = fresh.read(&path, 1 << 20).unwrap();
            assert!(
                published(&snapshot(&saved), &old, &new, began..=stamp()),
                "{name}: replay did not publish"
            );
            let end = after.encode_utf16().count() as u32;
            fresh
                .commit_transaction(&path, &replaced(&saved, end..end, suffix))
                .unwrap();
            let recovered = fresh.read(&path, 1 << 20).unwrap();
            assert_eq!(text(&recovered).2, format!("{after}{suffix}"));
            fs::write(
                output.join("recovered").join(format!("{name}.one")),
                recovered,
            )
            .unwrap();
            results.push(json!({"case":name,"path":path,"direction":direction,"command":command,"status":status,"occurrence":occurrence,
                "state":format!("{:?}",error.state),"visible":if visible {"after"} else {"before"},
                "retired_session_rejected":true,"exclusion_released":true,"fresh_commit_succeeded":true}));
            fs::write(
                output.join("results.json"),
                serde_json::to_vec_pretty(&results).unwrap(),
            )
            .unwrap();
        }
    }
    for state in ["NotCommitted", "Unknown", "Committed"] {
        assert!(results.iter().any(|result| result["state"] == state));
    }
    assert!(
        results
            .iter()
            .any(|result| result["state"] == "Unknown" && result["visible"] == "before")
    );
    assert!(
        results
            .iter()
            .any(|result| result["state"] == "Unknown" && result["visible"] == "after")
    );
    println!("{} message-loss cases passed", results.len());
}
