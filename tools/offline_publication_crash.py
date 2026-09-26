#!/usr/bin/env python3
"""Kill owned processes across local-cache/remote-file publication boundaries."""
import argparse
import cache_images
import hashlib
import json
import os
from pathlib import Path
import queue
import shutil
import signal
import sqlite3
import subprocess
import threading
import time

ROOT = Path(__file__).resolve().parent.parent
BINARY = ROOT / 'target/debug/examples/recovery_probe'
TOKEN = ' [offline-recovery]'


def run_command(binary, args, output):
    result = subprocess.run([str(binary), *map(str, args)], capture_output=True, text=True, timeout=60,
                            env={**os.environ, 'ONESTORE_RECOVERY_PAUSE': ''})
    output.with_suffix('.jsonl').write_text(result.stdout)
    output.with_suffix('.stderr').write_text(result.stderr)
    result.check_returncode()
    return [json.loads(line) for line in result.stdout.splitlines()]


def kill_at(binary, args, phase, output, receipt_window=None):
    events = queue.Queue()
    database = Path(args[1]) / 'cache.sqlite'
    with output.with_suffix('.jsonl').open('w') as log, output.with_suffix('.stderr').open('w') as error:
        process = subprocess.Popen([str(binary), *map(str, args)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=error, text=True,
                                   env={**os.environ, 'ONESTORE_RECOVERY_PAUSE': phase})
        def collect():
            try:
                for line in process.stdout:
                    log.write(line)
                    log.flush()
                    events.put(json.loads(line))
            finally:
                events.put(None)
        reader = threading.Thread(target=collect)
        reader.start()
        try:
            deadline = time.monotonic() + 60
            while True:
                event = events.get(timeout=max(0, deadline-time.monotonic()))
                assert event is not None, f'Process exited before {phase}'
                if event.get('event') == 'phase' and event['name'] == phase:
                    if receipt_window is not None:
                        assert args[0] == 'sync' and phase == 'publish-after'
                        assert receipt_window == 'wal'
                        # The receipt transaction is cut once its frames are in the log and
                        # its commit frame is not.
                        committed, _ = cache_images.wal_commits(database)
                        process.stdin.write('\n')
                        process.stdin.flush()
                        deadline = time.monotonic() + 5
                        while True:
                            commits, pending = cache_images.wal_commits(database)
                            if commits == committed and pending: break
                            assert commits == committed and process.poll() is None, 'Receipt transaction finished before the requested cut'
                            assert time.monotonic() < deadline, 'Receipt transaction window was not observed'
                    process.kill()
                    break
            assert process.wait(timeout=10) == -signal.SIGKILL, 'Expected an actual process kill'
            proof = {'pid': process.pid, 'exit': process.returncode, 'phase': phase}
            if receipt_window is not None:
                commits, pending = cache_images.wal_commits(database)
                proof.update(wal_commits_before=committed, wal_commits_after_kill=commits, uncommitted_frames=pending, receipt_window=receipt_window)
                assert commits == committed, 'Receipt transaction committed before the process died'
            output.with_suffix('.cut.json').write_text(json.dumps(proof, indent=2))
        finally:
            if process.poll() is None: process.kill()
            process.wait(timeout=10)
            process.stdin.close()
            reader.join(timeout=10)
            assert not reader.is_alive(), 'Trace reader did not finish'
            process.stdout.close()


def state(rows, original):
    found = [row for row in rows if row['event'] == 'state']
    assert len(found) == 1, 'Missing independent post-reopen state'
    result, = found
    assert result['local_text'] == original + TOKEN, 'Locally acknowledged text was lost or duplicated'
    assert result['remote_text'] in [original, original+TOKEN], 'Remote current text is partial, duplicated or invented'
    assert result['status'] in ('pending', 'uncertain', 'published'), 'Intent disappeared or became an unexplained conflict'
    if result['status'] == 'pending':
        assert result['revision'] is None, 'Unattempted intent acquired a publication identity'
    else:
        assert isinstance(result['revision'], str) and result['revision'], 'Attempted identity was lost'
        assert (result['revision'] == result['remote_revision']) == (result['remote_text'] == original+TOKEN), 'Publication identity disagrees with the visible effect'
    if result['status'] == 'published':
        assert result['pending'] == [] and result['remote_text'] == original+TOKEN
        assert result['revision'] == result['remote_revision'], 'Receipt identifies another remote revision'
    else:
        assert len(result['pending']) == 1, 'Unacknowledged intent was lost or duplicated'
        pending, = result['pending']
        assert pending['id'] == 1 and pending['replacement'] == TOKEN
        at = len(original.encode('utf-16-le')) // 2
        assert pending['range'] == [at, at], 'Durable intent range changed'
    return result


def save_image(output, data):
    digest = hashlib.sha256(data).hexdigest()
    path = output / 'images' / f'{digest}.one'
    if not path.exists(): path.write_bytes(data)
    return digest


def confirmation_only(events, before, after):
    assert not any(row['event'] == 'phase' and row['name'] == 'publish-before' for row in events)
    assert all(row['offset'] == 212 and row['bytes'] == 40 for row in events if row['event'] == 'write'), 'Recovery republished the remote edit'
    assert before[:212] == after[:212] and before[252:] == after[252:], 'Confirmation changed content or the transaction count'


def prepare_run(source, output):
    output.mkdir(parents=True, exist_ok=False)
    (output / 'images').mkdir()
    binary = output / 'recovery_probe'
    shutil.copyfile(BINARY, binary)
    binary.chmod(0o755)
    shutil.copyfile(__file__, output / Path(__file__).name)
    source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
    (output / 'run.json').write_text(json.dumps({'source': str(source), 'source_sha256': source_hash,
        'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'controller_sha256': hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}, indent=2))
    return binary, source_hash


def run(source, output):
    binary, source_hash = prepare_run(source, output)
    baseline = output / 'baseline'
    initialized = run_command(binary, ['init', baseline, source], output / 'baseline-init')
    original = next(row['remote_text'] for row in initialized if row['event'] == 'state')
    state(initialized, original)
    published = run_command(binary, ['sync', baseline], output / 'baseline-sync')
    assert state(published, original)['status'] == 'published'
    phases = [row['name'] for row in published if row['event'] == 'phase']
    assert len(phases) == len(set(phases)), 'Baseline has ambiguous phase names'
    assert 'publish-after' in phases and any(name.startswith('write-') for name in phases)
    confirmation = output / 'confirmation-baseline'
    run_command(binary, ['init', confirmation, source], output / 'confirmation-init')
    kill_at(binary, ['sync', confirmation], 'publish-after', output / 'confirmation-setup')
    confirmed = run_command(binary, ['sync', confirmation], output / 'confirmation-sync')
    confirmation_phases = [row['name'] for row in confirmed if row['event'] == 'phase']
    assert len(confirmation_phases) == len(set(confirmation_phases)) and 'confirm-after' in confirmation_phases
    assert state(confirmed, original)['status'] == 'published'
    cases = [('local-after', False), *((phase, False) for phase in phases), *((phase, True) for phase in confirmation_phases)]
    results = []
    for index, (phase, confirmation) in enumerate(cases):
        folder = output / f'case-{index:02}'
        trace = output / f'case-{index:02}-kill'
        if phase == 'local-after':
            kill_at(binary, ['init', folder, source], phase, trace)
        else:
            run_command(binary, ['init', folder, source], output / f'case-{index:02}-init')
            if confirmation:
                kill_at(binary, ['sync', folder], 'publish-after', output / f'case-{index:02}-setup')
            kill_at(binary, ['sync', folder], phase, trace)
        before = state(run_command(binary, ['inspect', folder], output / f'case-{index:02}-inspect'), original)
        before_bytes = (folder / 'remote.one').read_bytes()
        first = run_command(binary, ['sync', folder], output / f'case-{index:02}-recover')
        after = state(first, original)
        if before['status'] == 'uncertain' and before['remote_text'] == original:
            assert after['status'] == 'uncertain' and after['revision'] == before['revision'], 'Absent uncertain attempt was replayed'
            assert not any(row['event'] == 'write' for row in first)
        else:
            assert after['status'] == 'published'
            if before['status'] != 'pending':
                assert after['revision'] == before['revision'], 'Recovery published another revision'
                confirmation_only(first, before_bytes, (folder / 'remote.one').read_bytes())
        remote_hash = hashlib.sha256((folder / 'remote.one').read_bytes()).hexdigest()
        repeated = run_command(binary, ['sync', folder], output / f'case-{index:02}-repeat')
        assert state(repeated, original) == after, 'Repeated recovery changed durable intent state'
        assert not any(row['event'] == 'write' for row in repeated), 'Repeated recovery published again'
        assert hashlib.sha256((folder / 'remote.one').read_bytes()).hexdigest() == remote_hash
        remote_image = save_image(output, (folder / 'remote.one').read_bytes())
        connection = sqlite3.connect(folder / 'cache.sqlite')
        try:
            assert connection.execute('PRAGMA quick_check').fetchall() == [('ok',)]
            assert connection.execute('PRAGMA foreign_key_check').fetchall() == []
            # The image the queue applies to; the local text is the probe's, checked above.
            base_image = save_image(output, cache_images.image(connection))
        finally:
            connection.close()
        result = {'case': index, 'phase': phase, 'during_confirmation': confirmation, 'before_status': before['status'], 'after_status': after['status'],
                  'visible_before_recovery': before['remote_text'] != original, 'remote_image': remote_image, 'base_image': base_image,
                  'remote_text': after['remote_text'], 'local_text': after['local_text']}
        results.append(result)
        (output / 'results.json').write_text(json.dumps(results, indent=2))
        print(json.dumps({key: value for key, value in result.items() if not key.endswith('_text')}), flush=True)
    assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash, 'The frozen source changed'
    summary = {'cases': len(results), 'process_kills': 1 + len(results) + sum(confirmation for _, confirmation in cases), 'uncertain_absent_preserved': sum(row['after_status']=='uncertain' for row in results),
               'durable_receipts': sum(row['after_status']=='published' for row in results), 'unique_images': len(list((output/'images').glob('*.one')))}
    (output/'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)


def receipt_windows(source, output):
    binary, source_hash = prepare_run(source, output)
    results = []
    for window in ('wal',):
        folder = output / window
        initialized = run_command(binary, ['init', folder, source], output / (window+'-init'))
        original = next(row['remote_text'] for row in initialized if row['event'] == 'state')
        state(initialized, original)
        kill_at(binary, ['sync', folder], 'publish-after', output / (window+'-kill'), receipt_window=window)
        before = state(run_command(binary, ['inspect', folder], output / (window+'-inspect')), original)
        assert before['status'] == 'uncertain' and before['remote_text'] == original+TOKEN, 'Log recovery lost the pending confirmation'
        before_bytes = (folder / 'remote.one').read_bytes()
        recovered = run_command(binary, ['sync', folder], output / (window+'-recover'))
        after = state(recovered, original)
        assert after['status'] == 'published' and before['revision'] == after['revision']
        confirmation_only(recovered, before_bytes, (folder / 'remote.one').read_bytes())
        repeated = run_command(binary, ['sync', folder], output / (window+'-repeat'))
        assert state(repeated, original) == after and not any(row['event']=='write' for row in repeated)
        digest = save_image(output, (folder/'remote.one').read_bytes())
        results.append({'receipt_window':window, 'remote_image':digest, 'remote_text':after['remote_text'], 'revision':after['revision']})
        print(json.dumps({'receipt_window':window, 'status':after['status'], 'remote_image':digest}), flush=True)
    assert hashlib.sha256(source.read_bytes()).hexdigest() == source_hash
    (output/'results.json').write_text(json.dumps(results, indent=2))
    (output/'summary.json').write_text(json.dumps({'process_kills':len(results), 'recovered_receipts':len(results), 'republished_edits':0}, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('source', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--receipt-windows', action='store_true')
    args = parser.parse_args()
    (receipt_windows if args.receipt_windows else run)(args.source.resolve(), args.output.resolve())
