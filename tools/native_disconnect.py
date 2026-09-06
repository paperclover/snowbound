"""Interrupt an owned mixed append workload and require progress after reconnection."""
import json
import shlex
import subprocess
import time

from native_runner import windows
import linux_vm
from verify_smb_overlap import verify


def interrupt(output, clients, sequences, processes):
    config = json.loads((output / 'run.json').read_text())
    server = config['server']
    samples = []
    subprocess.run(linux_vm.ssh_argv(server, 'cat > /tmp/verify_smb_overlap.py'),
                   input=(output / 'harness/verify_smb_overlap.py').read_bytes(), check=True)

    def counts(transport=False):
        result = {}
        for actor in processes:
            data = (output / 'rust' / (actor + '.jsonl')).read_text()
            events = [json.loads(line) for line in data[:data.rfind('\n') + 1].splitlines()]
            names = ('transport_read_error', 'transport_commit_error') if transport else ('commit' if actor.startswith('w') else 'read',)
            result[actor] = sum(event['event'] in names for event in events)
        return result

    def wait_for(predicate, message):
        deadline = time.monotonic() + 90
        while not predicate():
            assert all(process.poll() is None for process in processes.values()), 'A Rust client exited during the interruption campaign'
            if time.monotonic() > deadline: raise TimeoutError(message)
            time.sleep(.1)

    def ssh(command):
        result = linux_vm.run_ssh(server, command, timeout=15)
        with (output / 'disconnect-server.jsonl').open('a') as stream:
            stream.write(json.dumps({'command': command, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}) + '\n')
        result.check_returncode()
        return result.stdout

    def phase(control):
        text = json.dumps(control)
        ssh("printf '%s' '" + text + "' > /tmp/smb-control.tmp && mv /tmp/smb-control.tmp /tmp/smb-control.json")
        wait_for(lambda: text in ssh("grep -F '\"control\":' /tmp/smb-trace.jsonl | tail -n 1"), 'Proxy did not acknowledge the disconnect phase')

    previous = {actor: 0 for actor in processes}
    for cycle in range(2):
        wait_for(lambda: all(value >= previous[actor] + 3 for actor, value in counts().items()), 'Clients made no progress before the interruption')
        before = counts()
        native = []
        for actor, (client, sequence) in enumerate(zip(clients, sequences)):
            capture = output / f'disconnect-{cycle}-n{actor}.jsonl'
            result = windows.do_get(f'C:\\one-tests\\runs\\capture\\outbox\\{sequence}\\events.jsonl', capture, client['name'])
            assert not result.get('error'), result
            data = capture.read_text(encoding='utf-8-sig')
            rows = [json.loads(line) for line in data[:data.rfind('\n') + 1].splitlines()]
            assert 0 < len(rows) < config['stress_operations'], 'A native writer was inactive before the interruption'
            native.append(len(rows))
        before_errors = counts(transport=True)
        assert all(process.poll() is None for process in processes.values()), 'A Rust client finished before the interruption'
        try:
            phase({'phase': f'disconnect-{cycle}', 'cut': 9, 'peer': '10.0.2.2', 'offset': 96,
                   'direction': 'request' if cycle == 0 else 'response'})
            wait_for(lambda: f'"phase": "disconnect-{cycle}"' in ssh("grep -F '\"control\":' /tmp/smb-trace.jsonl | tail -n 1")
                     and int(ssh("grep -c '\"cut\": {' /tmp/smb-trace.jsonl || true").strip()) == cycle + 1,
                     'The planned write interruption did not occur')
            time.sleep(3)
        finally:
            phase({'phase': f'reconnected-{cycle}'})
        wait_for(lambda: all(value >= before[actor] + 3 for actor, value in counts().items()), 'A client failed to progress after reconnecting')
        script = '\n'.join([
            'import json', 'from verify_smb_overlap import verify, PendingOverlap',
            "data = open('/tmp/smb-trace.jsonl').read()",
            "events = [json.loads(line) for line in data[:data.rfind('\\n') + 1].splitlines()]",
            'try:', f"    result = verify(events, phase='reconnected-{cycle}')",
            "except PendingOverlap: result = None", 'print(json.dumps(result))'])
        wait_for(lambda: json.loads(ssh('cd /tmp && python3 -c ' + shlex.quote(script))) is not None,
                 'Native and Rust guarded I/O did not overlap after reconnection')
        previous = counts()
        samples.append({'cycle': cycle, 'before': before, 'after': previous, 'native_before': native,
                        'before_errors': before_errors, 'after_errors': counts(transport=True)})
        (output / 'disconnect-progress.json').write_text(json.dumps(samples, indent=2))


def verify_disconnect(output):
    config = json.loads((output / 'run.json').read_text())
    assert config['stress_clients'] + config['rust_writers'] + config['rust_readers'] >= 12
    samples = json.loads((output / 'disconnect-progress.json').read_text())
    assert [sample['cycle'] for sample in samples] == [0, 1]
    actors = {f'w{i}' for i in range(config['rust_writers'])} | {f'r{i}' for i in range(config['rust_readers'])}
    events = [json.loads(line) for line in (output / 'smb-trace.jsonl').read_text().splitlines()]
    assert sum('cut' in event for event in events) == 2, 'Unexpected number of connection interruptions'
    errors = {}
    progress = {}
    progress_limit = 120
    started = (output / 'rust/start').stat().st_mtime_ns // 1000
    stopped = (output / 'rust/stop').stat().st_mtime_ns // 1000

    def max_gap(actor, times, units):
        assert len(times) > 1, 'A client made no progress'
        gaps = [(end - begin) / units for begin, end in zip(times, times[1:])]
        assert all(0 <= gap <= progress_limit for gap in gaps), f'{actor}: client progress stalled or went backwards'
        return max(gaps)

    for i in range(config['stress_clients']):
        rows = [json.loads(line) for line in (output / f'n{i}/stress-events.jsonl').read_text(encoding='utf-8-sig').splitlines()]
        assert len(rows) == config['stress_operations'], 'A native writer did not finish'
        progress[f'n{i}'] = max_gap(f'n{i}', [rows[0]['update_started_ticks'], *[row['updated_ticks'] for row in rows]], 10**7)
    uncertain = set()
    for actor in sorted(actors):
        rows = [json.loads(line) for line in (output / 'rust' / (actor + '.jsonl')).read_text().splitlines()]
        errors[actor] = sum(row['event'] in ('transport_read_error', 'transport_commit_error') for row in rows)
        assert errors[actor] and any(row['event'] == 'transport_connected' for row in rows), 'A Rust client did not exercise reconnection'
        times = [started, *[row['finished_us'] for row in rows if row['event'] == ('commit' if actor.startswith('w') else 'read')]]
        if actor.startswith('r') and len(times) > 1: times.append(max(times[-1], stopped))
        progress[actor] = max_gap(actor, times, 10**6)
        pending = None
        for row in rows:
            if row['event'] == 'transport_commit_error': pending = row
            if row['event'] != 'transport_reconciled': continue
            assert pending is not None and pending['token'] == row['token']
            if row['published']: assert row.get('flush_confirmed'), 'Visible recovery lacks a durable acknowledgement'
            if pending['state'] == 'Unknown': uncertain.add('after' if row['published'] else 'before')
            pending = None
    assert uncertain == {'before', 'after'}, 'The mixed workload did not resolve both uncertain outcomes'
    overlap = []
    for sample in samples:
        assert all(set(sample[field]) == actors for field in ('before', 'after', 'before_errors', 'after_errors'))
        assert all(sample['after'][actor] >= value + 3 for actor, value in sample['before'].items())
        assert all(sample['after_errors'][actor] > value for actor, value in sample['before_errors'].items()), 'A client did not encounter this interruption'
        assert len(sample['native_before']) == config['stress_clients']
        assert all(0 < value < config['stress_operations'] for value in sample['native_before'])
        end = next((i for i, event in enumerate(events) if event.get('control', {}).get('phase') == f'disconnect-{sample["cycle"] + 1}'), len(events))
        overlap.append(verify(events[:end], phase=f'reconnected-{sample["cycle"]}'))
    return {'interruptions': 2, 'transport_errors': errors, 'uncertain_outcomes': sorted(uncertain), 'resumed_overlap': overlap,
            'max_progress_gap_seconds': progress, 'progress_limit_seconds': progress_limit}
