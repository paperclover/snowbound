#!/usr/bin/env python3
"""Interrupt owned offline-cache writers and verify every retained intent and image."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import time


def run(binary, output):
    output.mkdir(parents=True, exist_ok=False)
    cache = output / "cache.sqlite"
    source = os.environ.get("ONESTORE_CACHE_PROBE_SOURCE")
    source_hash = hashlib.sha256(Path(source).read_bytes()).hexdigest() if source else None
    payload_bytes = int(os.environ.get("ONESTORE_CACHE_PROBE_BYTES", 2 * 1024 * 1024))
    assert 0 < payload_bytes <= 2 * 1024 * 1024
    (output / "run.json").write_text(json.dumps({
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "controller_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "payload_bytes": payload_bytes,
        "operation": os.environ.get("ONESTORE_CACHE_PROBE_OPERATION", "text"),
        "source": source,
        "source_sha256": source_hash,
    }, indent=2))
    subprocess.run([binary, "init", cache], check=True, timeout=30)
    retained = []
    retained_ids = []
    results = []
    delays = [None, "ack", "unack", "journal", "database-write", 0, .001, .01, .02, .04, .08, .16, .32, .64, 1.28, 2.56, None, "ack"]
    for operation, delay in enumerate(delays, 1):
        with (output / f"owner-{operation}.stderr").open("w") as stderr:
            owner = subprocess.Popen([binary, "edit", cache], stdin=subprocess.PIPE,
                                     stdout=subprocess.PIPE, stderr=stderr, text=True, bufsize=1)
            try:
                def expect(prefix):
                    if not select.select([owner.stdout], [], [], 60)[0]:
                        raise TimeoutError(prefix)
                    actual = owner.stdout.readline().strip()
                    assert actual == prefix or actual.startswith(prefix + " "), (prefix, actual, owner.poll())
                    return actual

                expect("ready")
                contender = subprocess.run([binary, "read", cache], capture_output=True, text=True, timeout=15)
                (output / f"contender-{operation}.stderr").write_text(contender.stderr)
                assert contender.returncode != 0 and "DatabaseBusy" in contender.stderr, contender
                before_write = cache.stat().st_mtime_ns
                owner.stdin.write(f"{operation} {'unack' if delay == 'unack' else 'ack'}\n")
                owner.stdin.flush()
                expect(f"editing {operation}")
                acknowledgement = None
                journal_observation = None
                if delay == "ack":
                    acknowledgement = expect(f"ack {operation}")
                elif delay == "unack":
                    expect(f"durable {operation}")
                elif delay in ("journal", "database-write"):
                    deadline = time.monotonic() + 10
                    journal = cache.with_name(cache.name + "-journal")
                    while time.monotonic() < deadline:
                        try:
                            with journal.open("rb") as active:
                                header = active.read(28)
                            size = journal.stat().st_size
                            database_changed = cache.stat().st_mtime_ns != before_write
                            if (header[:8] == bytes.fromhex("d9d505f920a163d7") and
                                    (delay == "journal" or database_changed)):
                                journal_observation = {"header": header.hex(), "bytes": size,
                                                       "database_changed": database_changed}
                                break
                        except FileNotFoundError:
                            pass
                        time.sleep(.0001)
                    assert journal_observation, f"No active {delay} phase observed"
                elif delay is not None:
                    time.sleep(delay)
                owner.kill()
                assert owner.wait(timeout=10) == -signal.SIGKILL, "Writer did not terminate at the requested process cut"
                extra = owner.stdout.read()
                if f"ack {operation} " in extra:
                    acknowledgement = extra.strip()
                read = subprocess.run([binary, "read", cache], check=True, capture_output=True,
                                      text=True, timeout=60)
                actual = json.loads(read.stdout)
                operations = actual["operations"]
                ids = actual["ids"]
                assert operations in (retained, retained + [operation]), (retained, operation, actual)
                assert ids[:len(retained_ids)] == retained_ids, (retained_ids, actual)
                if acknowledgement:
                    assert operations[-1] == operation
                    assert ids[-1] == int(acknowledgement.split()[2])
                if delay == "unack":
                    assert operations[-1] == operation and acknowledgement is None
                assert actual["complete_payloads"]
                results.append({"operation": operation, "delay": delay,
                                "process_exit": owner.returncode,
                                "acknowledged": acknowledgement is not None,
                                "retained": operation in operations,
                                "retained_operations": operations, "ids": ids,
                                "section_bytes": actual["section_bytes"],
                                "complete_payloads": True, "exclusive_owner": True,
                                "journal_observation": journal_observation})
                retained, retained_ids = operations, ids
                (output / "results.json").write_text(json.dumps(results, indent=2))
                print(json.dumps(results[-1]), flush=True)
            finally:
                if owner.poll() is None:
                    owner.kill()
                    owner.wait(timeout=10)
                owner.stdin.close()
                owner.stdout.close()
    assert any(row["acknowledged"] for row in results)
    assert any(row["retained"] and not row["acknowledged"] for row in results)
    assert any(not row["retained"] for row in results)
    subprocess.run([binary, "read", cache, output / "recovered.one"], check=True, timeout=60)
    if source:
        assert hashlib.sha256(Path(source).read_bytes()).hexdigest() == source_hash
    print(f"Passed {len(results)} offline-cache process interruptions", flush=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    parser.add_argument("--binary", type=Path, default=Path(__file__).resolve().parents[1] / "target/release/examples/cache_probe")
    args = parser.parse_args()
    run(args.binary.resolve(), args.output.resolve())
