#!/usr/bin/env python3
"""Race independent Rust clients and verify every observed append against committed operations."""
import argparse
from collections import Counter
from contextlib import contextmanager
import hashlib
import json
import os
from pathlib import Path
import subprocess
import time

ROOT = Path(__file__).resolve().parent.parent
CLIENT = ROOT / 'target/debug/examples/concurrent_client'


def verify(logs, initial_transaction, writers, operations, edit=False):
    commits = [(actor, event) for actor, events in logs.items() for event in events if event['event'] == 'commit']
    assert Counter(actor for actor, _ in commits) == {f'w{i}': operations for i in range(writers)}, 'Missing or extra acknowledged operations'
    ordered = sorted(commits, key=lambda pair: pair[1]['source_transaction'])
    versions = {initial_transaction: 'Concurrent edits:'}
    seen = set()
    for i, (actor, event) in enumerate(ordered):
        transaction = initial_transaction + i
        assert event['source_transaction'] == transaction, 'Two writers committed the same snapshot or skipped a transaction'
        assert event['token'] == f' [{actor}:{event["operation"]}]', 'Committed token differs from intended operation'
        assert (actor, event['operation']) not in seen, 'An operation committed twice'
        seen.add((actor, event['operation']))
        if edit:
            intent, = [e for e in logs[actor] if e['event'] == 'intent' and e['attempt'] == event['attempt']]
            assert intent['before'] == versions[transaction] and intent['source_transaction'] == transaction, 'Intent used another snapshot'
            assert intent['operation'] == event['operation'] and intent['token'] == event['token'], 'Acknowledgement differs from intent'
            assert intent['replacement'] == ' café 🦀' + event['token'], 'Unexpected replacement text'
            start, end = intent['range']
            units = intent['before'].encode('utf-16-le')
            assert len(versions[initial_transaction]) <= start <= end <= len(units) // 2, 'Invalid edit range'
            units[:start * 2].decode('utf-16-le')
            units[end * 2:].decode('utf-16-le')
            versions[transaction + 1] = (units[:start * 2] + intent['replacement'].encode('utf-16-le') + units[end * 2:]).decode('utf-16-le')
        else:
            versions[transaction + 1] = versions[transaction] + event['token']
    observations = 0
    intervals = []
    for actor, events in logs.items():
        assert events and events[0]['event'] == 'ready' and events[-1]['event'] == 'done', 'Client did not finish'
        previous = initial_transaction
        for event in events:
            if event['event'] in ('commit', 'retry'):
                intervals.append((actor, event['started_us'], event['finished_us']))
            if event['event'] != 'read':
                continue
            transaction = event['transaction']
            assert transaction >= previous, 'A reader went backwards'
            previous = transaction
            assert transaction in versions and event['text'] == versions[transaction], 'A reader observed a lost, partial, duplicated or invented edit'
            for _, commit in ordered:
                published = commit['source_transaction'] + 1
                if commit['finished_us'] < event['started_us']:
                    assert transaction >= published, 'Read missed an already acknowledged commit'
                if commit['started_us'] > event['finished_us']:
                    assert transaction < published, 'Read observed a future commit'
            observations += 1
    overlap = sum(a != b and max(start, other_start) < min(end, other_end)
                  for i, (a, start, end) in enumerate(intervals)
                  for b, other_start, other_end in intervals[i + 1:])
    assert overlap, 'Writer calls never overlapped'
    return {'commits': len(commits), 'observations': observations, 'overlapping_writer_calls': overlap,
            'retries': sum(e['event'] == 'retry' for events in logs.values() for e in events),
            'final_transaction': initial_transaction + len(commits), 'final_text': versions[max(versions)]}


@contextmanager
def running_clients(output, source, writers, readers, operations, seed, timeout=600, edit=False, executable=CLIENT, environment=None, reader_executable=None):
    if not 0 < timeout < 2**64 / 1000:
        raise ValueError('Choose a finite positive client timeout.')
    timeout_ms = int(timeout * 1000)
    if not 0 < timeout_ms < 2**64:
        raise ValueError('Client timeout does not fit the subprocess clock.')
    environment = {**(os.environ if environment is None else environment), 'ONESTORE_CLIENT_TIMEOUT_MS': str(timeout_ms)}
    start, stop = output / 'start', output / 'stop'
    manifest = {'writers': writers, 'readers': readers, 'operations': operations,
        'seed': seed, 'edit': edit, 'timeout_ms': timeout_ms, 'client_sha256': hashlib.sha256(executable.read_bytes()).hexdigest(), 'executable': str(executable)}
    if reader_executable is not None:
        manifest.update(reader_executable=str(reader_executable), reader_sha256=hashlib.sha256(reader_executable.read_bytes()).hexdigest())
    (output / 'clients.json').write_text(json.dumps(manifest, indent=2))
    processes = {}
    streams = []
    try:
        for mode, count in [('write', writers), ('read', readers)]:
            client = reader_executable if mode == 'read' and reader_executable is not None else executable
            for i in range(count):
                actor = mode[0] + str(i)
                out = (output / f'{actor}.jsonl').open('w')
                err = (output / f'{actor}.stderr').open('w')
                streams.extend([out, err])
                processes[actor] = subprocess.Popen([client, 'edit' if edit and mode == 'write' else mode, source, actor, str(operations), start, stop,
                    str(seed + i + (10000 if mode == 'read' else 0))], stdout=out, stderr=err, env=environment)
        deadline = time.monotonic() + timeout
        while not all((output / f'{actor}.jsonl').stat().st_size for actor in processes):
            if any(p.poll() is not None for p in processes.values()) or time.monotonic() > deadline:
                raise RuntimeError('A client failed before the start barrier.')
            time.sleep(.01)
        yield processes
        while any(p.poll() is None for p in processes.values()):
            failed = {actor: p.returncode for actor, p in processes.items() if p.returncode not in (None, 0)}
            if failed or time.monotonic() > deadline:
                raise RuntimeError(f'Concurrent clients failed or timed out: {failed}')
            if all(p.poll() == 0 for actor, p in processes.items() if actor.startswith('w')):
                stop.touch(exist_ok=True)
            time.sleep(.05)
        assert all(p.returncode == 0 for p in processes.values()), 'A client exited unsuccessfully'
    finally:
        for process in processes.values():
            if process.poll() is None:
                process.terminate()
        for process in processes.values():
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for stream in streams:
            stream.close()
        (output / 'teardown.json').write_text(json.dumps({actor: p.returncode for actor, p in processes.items()}, indent=2))


def run(output, writers, readers, operations, seed, notebook=None, timeout=600, edit=False):
    if min(writers, readers, operations) < 1 or writers < 2:
        raise ValueError('Choose at least two writers, one reader and one operation.')
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    notebook = output / 'notebook' if notebook is None else notebook.resolve()
    notebook.mkdir()
    source = notebook / 'synthetic.one'
    seed_file = output / 'initial.one'
    subprocess.run([CLIENT, 'init', seed_file], check=True)
    initial = seed_file.read_bytes()
    with source.open('xb') as stream:
        stream.write(initial)
        stream.flush()
        os.fsync(stream.fileno())
    start, stop = output / 'start', output / 'stop'
    (output / 'run.json').write_text(json.dumps({'initial_sha256': hashlib.sha256(initial).hexdigest(),
        'generator_sha256': hashlib.sha256(CLIENT.read_bytes()).hexdigest()}, indent=2))
    started = time.monotonic()
    with running_clients(output, source, writers, readers, operations, seed, timeout, edit) as processes:
        start.touch()
    logs = {actor: [json.loads(line) for line in (output / f'{actor}.jsonl').read_text().splitlines()] for actor in processes}
    result = verify(logs, int.from_bytes(initial[96:100], 'little'), writers, operations, edit)
    final = subprocess.run([CLIENT, 'read', source, 'final', '1', start, stop, str(seed)], check=True, capture_output=True, text=True)
    (output / 'final.jsonl').write_text(final.stdout)
    final_read, = [json.loads(line) for line in final.stdout.splitlines() if json.loads(line)['event'] == 'read']
    assert final_read['text'] == result['final_text'] and final_read['transaction'] == result['final_transaction'], 'Final file lost an acknowledged edit'
    result['elapsed_seconds'] = time.monotonic() - started
    final_bytes = source.read_bytes()
    result['final_sha256'] = hashlib.sha256(final_bytes).hexdigest()
    (output / 'final.one').write_bytes(final_bytes)
    (output / 'result.json').write_text(json.dumps(result, indent=2))
    print(json.dumps({k: v for k, v in result.items() if k != 'final_text'}), flush=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--writers', type=int, default=4)
    parser.add_argument('--readers', type=int, default=3)
    parser.add_argument('--operations', type=int, default=30)
    parser.add_argument('--seed', type=int, default=1)
    parser.add_argument('--notebook-dir', type=Path)
    parser.add_argument('--timeout', type=int, default=600)
    parser.add_argument('--edit', action='store_true')
    args = parser.parse_args()
    run(args.output, args.writers, args.readers, args.operations, args.seed, args.notebook_dir, args.timeout, args.edit)
