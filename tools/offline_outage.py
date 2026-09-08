"""Require durable local progress during a confirmed outage of the owned SMB proxy."""
import hashlib
import json
from pathlib import Path
import shlex
import time

from native_runner import windows
import linux_vm
from verify_smb_overlap import verify
from offline_document_history import operation_kinds


def interrupt(output, clients, sequences, processes):
    config = json.loads((output / 'run.json').read_text())
    folder = output / 'rust'
    samples = {}

    def logs():
        result = {}
        for actor in processes:
            text = (folder / f'{actor}.jsonl').read_text()
            result[actor] = [json.loads(line) for line in text[:text.rfind('\n') + 1].splitlines()]
        return result

    def wait_for(predicate, message, timeout=90):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert all(p.poll() in (None, 0) for p in processes.values()), 'A client failed during the offline outage'
            if time.monotonic() > deadline: raise TimeoutError(message)
            time.sleep(.05)

    def ssh(command):
        result = linux_vm.run_ssh(config['server'], command, timeout=15)
        with (output / 'offline-outage-server.jsonl').open('a') as stream:
            stream.write(json.dumps({'command': command, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}) + '\n')
        result.check_returncode()
        return result.stdout

    def phase(control):
        text = json.dumps(control)
        ssh("printf '%s' " + shlex.quote(text) + ' > /tmp/smb-control.tmp && mv /tmp/smb-control.tmp /tmp/smb-control.json')
        wait_for(lambda: text in ssh("grep -F '\"control\":' /tmp/smb-trace.jsonl | tail -n 1"), 'Proxy did not acknowledge the outage phase')

    def native_counts(label):
        counts = []
        for actor, (client, sequence) in enumerate(zip(clients, sequences)):
            capture = output / f'offline-{label}-n{actor}.jsonl'
            result = windows.do_get(f'C:\\one-tests\\runs\\capture\\outbox\\{sequence}\\events.jsonl', capture, client['name'])
            assert not result.get('error'), result
            text = capture.read_text(encoding='utf-8-sig')
            rows = [json.loads(line) for line in text[:text.rfind('\n') + 1].splitlines()]
            assert 0 < len(rows) < config['stress_operations'], 'Native client was inactive during the outage campaign'
            counts.append(len(rows))
        return counts

    writers = [actor for actor in processes if actor.startswith('w')]
    readers = [actor for actor in processes if actor.startswith('r')]
    if config.get('offline_lost_reply'):
        isolated = config.get('offline_client_reply', False)
        def counts():
            return {actor: sum(row['event'] == ('remote_receipt' if actor in writers else 'read') for row in rows)
                    for actor, rows in logs().items()}
        if config.get('document_operations'):
            wait_for(lambda: all((folder / f'offline-paused-{actor}').exists() for actor in writers)
                     and all(counts()[actor] > 0 for actor in readers), 'Clients did not reach the formatting publication barrier')
        else:
            wait_for(lambda: all(value >= 3 for value in counts().values()), 'Clients made no progress before the reply cut')
        samples['before'] = counts()
        samples['native_before'] = native_counts('reply-before')
        try:
            control = {'phase': 'offline-reply-cut', 'cut': 9, 'peer': '10.0.2.2', 'offset': 96, 'direction': 'response'}
            if isolated:
                control['scope'] = 'connection'
                (folder / 'offline-paused-w0.isolate').touch()
            phase(control)
            if config.get('document_operations'):
                samples['format_released_us'] = time.time_ns() // 1000
                (folder / 'offline-paused-w0.resume').touch()
            wait_for(lambda: int(ssh("grep -c '\"cut\": {' /tmp/smb-trace.jsonl || true").strip()) == 1,
                     'The publication reply was not interrupted')
            samples['down_started_us'] = time.time_ns() // 1000
            if isolated:
                wait_for(lambda: any(row['event'] == 'confirmation_paused' for row in logs()['w0']),
                         'The disconnected writer did not retain its uncertain publication')
                for actor in writers[1:]: (folder / f'offline-paused-{actor}.resume').touch()
                wait_for(lambda: all(value >= samples['before'][actor] + 3 for actor, value in counts().items() if actor != 'w0'),
                         'Peers did not advance while the writer was disconnected')
                wait_for(lambda: (folder / 'offline-retired.one').exists(),
                         'Native maintenance did not retire the isolated revision')
            time.sleep(3)
            if isolated: samples['during'] = counts()
            samples['native_during'] = native_counts('reply-during')
        finally:
            samples['up_started_us'] = time.time_ns() // 1000
            phase({'phase': 'offline-reply-reconnected'})
            if config.get('document_operations'):
                for actor in writers: (folder / f'offline-paused-{actor}.resume').touch()
            if isolated: (folder / 'offline-paused-w0.confirmation-resume').touch()
            (output / 'offline-lost-reply-progress.json').write_text(json.dumps(samples, indent=2))
        wait_for(lambda: all(value >= samples['before'][actor] + 3 for actor, value in counts().items()),
                 'A client failed to progress after the lost publication reply', timeout=120)
        samples['after'] = counts()
        (output / 'offline-lost-reply-progress.json').write_text(json.dumps(samples, indent=2))
        return
    wait_for(lambda: all((folder / f'offline-paused-{actor}').exists() for actor in writers)
             and (not config.get('document_operations') or all(
                 sum(row['event'] == 'local_document_commit' for row in logs()[actor]) == len(operation_kinds(logs()[actor])) for actor in writers))
             and all(any(row['event'] == 'read' for row in logs()[actor]) for actor in readers),
             'Clients did not reach the pre-publication outage barrier')
    samples['native_before'] = native_counts('before')
    samples['reader_errors_before'] = {actor: sum(row['event'] == 'transport_read_error' for row in logs()[actor]) for actor in readers}
    try:
        phase({'phase': 'offline-down', 'mode': 'down'})
        samples['down_started_us'] = time.time_ns() // 1000
        (folder / 'offline-outage-down').touch()
        wait_for(lambda: all(sum(row['event'] == 'local_commit' for row in events) == 8 for actor, events in logs().items() if actor in writers)
                 and (not config.get('document_operations') or all(
                     sum(row['event'] == 'local_document_commit' for row in logs()[actor]) == 8 * len(operation_kinds(logs()[actor])) for actor in writers))
                 and all(sum(row['event'] == 'transport_read_error' for row in logs()[actor]) > samples['reader_errors_before'][actor] for actor in readers),
                 'Local queues or disconnected readers failed to progress during the outage')
        time.sleep(3)
        samples['native_during'] = native_counts('during')
        samples['up_started_us'] = time.time_ns() // 1000
    finally:
        phase({'phase': 'offline-reconnected'})
        (folder / 'offline-outage-resumed').touch()
        (output / 'offline-outage-progress.json').write_text(json.dumps(samples, indent=2))
    wait_for(lambda: all(sum(row['event'] == 'remote_receipt' for row in logs()[actor]) >= 3 for actor in writers)
             and all(sum(row['event'] == 'read' and row['started_us'] > samples['up_started_us'] for row in logs()[actor]) >= 3 for actor in readers),
             'A client failed to progress after the offline outage')


def native_progress(output, config, sample):
    down, up = sample['down_started_us'], sample['up_started_us']
    assert len(sample['native_before']) == len(sample['native_during']) == config['stress_clients']
    native = list(zip(sample['native_before'], sample['native_during']))
    assert all(0 < before < during < config['stress_operations'] for before, during in native), 'Native local edits did not advance during the outage'
    clocks = json.loads((output / 'clocks.json').read_text())
    native_inside = []
    assert len(clocks) == config['stress_clients']
    for index, clock in enumerate(clocks):
        low = min(clock['native_minus_host_us'][0], clock['after_native_minus_host_us'][0])
        high = max(clock['native_minus_host_us'][1], clock['after_native_minus_host_us'][1])
        rows = [json.loads(line) for line in (output / f'n{index}/stress-events.jsonl').read_text(encoding='utf-8-sig').splitlines()]
        inside = sum((row['update_started_ticks'] - 621355968000000000) // 10 - high > down
                     and (row['updated_ticks'] - 621355968000000000) // 10 - low < up for row in rows)
        assert inside, 'Native timestamps do not establish local edits inside the confirmed outage'
        native_inside.append(inside)
    return native_inside


def verify_outage(output):
    config = json.loads((output / 'run.json').read_text())
    assert config['offline'] and config['offline_outage'] and config['embedded_smb']
    assert config['stress_clients'] + config['rust_writers'] + config['rust_readers'] >= 12
    sample = json.loads((output / 'offline-outage-progress.json').read_text())
    down, up = sample['down_started_us'], sample['up_started_us']
    assert up - down >= 3_000_000, 'Confirmed outage lasted less than three seconds'
    native_inside = native_progress(output, config, sample)
    queues, reconnects = {}, {}
    for mode, count in [('w', config['rust_writers']), ('r', config['rust_readers'])]:
        for index in range(count):
            actor = f'{mode}{index}'
            events = [json.loads(line) for line in (output / 'rust' / f'{actor}.jsonl').read_text().splitlines()]
            assert events[0]['event'] == 'ready' and events[-1]['event'] == 'done'
            connected = [row for row in events if row['event'] == 'transport_connected']
            assert any(row['at_us'] > up for row in connected), 'No fresh transport after outage'
            reconnects[actor] = len(connected)
            if mode == 'r':
                assert sum(row['event'] == 'transport_read_error' for row in events) > sample['reader_errors_before'][actor]
                assert sum(row['event'] == 'read' and row['started_us'] > up for row in events) >= 3
                continue
            local = [row for row in events if row['event'] == 'local_commit']
            assert len(local) == config['stress_operations']
            assert local[0]['finished_us'] < down
            queued = [row for row in local if down < row['started_us'] <= row['finished_us'] < up]
            assert [row['operation'] for row in queued] == list(range(1, 8)), 'Seven local edits were not accepted while SMB was down'
            queues[actor] = len(queued)
            paused = [row for row in events if row['event'] == 'publication_paused']
            assert len(paused) == 1 and paused[0]['at_us'] < down
            attempts = [row for row in events if row['event'] == 'remote_attempt']
            assert attempts and all(row['started_us'] > up for row in attempts), 'Publication escaped the outage barrier'
            assert attempts[0]['state'] == 'NotCommitted' and attempts[0]['revision'] == paused[0]['revision'], 'Disconnected pre-I/O attempt was not safely rejected'
            assert all(row['state'] != 'Unknown' for row in attempts), 'Unexpected uncertain publication requires separate recovery evidence'
            receipts = [row for row in events if row['event'] == 'remote_receipt']
            assert len(receipts) == config['stress_operations'] and all(row['at_us'] > up for row in receipts)
            if config.get('document_operations'):
                kinds = operation_kinds(events)
                edits = [row for row in events if row['event'] == 'local_document_commit']
                assert [(row['operation'], row['kind']) for row in edits] == [
                    (operation, kind) for operation in range(config['stress_operations']) for kind in kinds]
                assert all(row['finished_us'] < down for row in edits[:len(kinds)]), 'Initial document edits missed the outage barrier'
                queued = [row for row in edits if down < row['started_us'] <= row['finished_us'] < up]
                assert [(row['operation'], row['kind']) for row in queued] == [
                    (operation, kind) for operation in range(1, 8) for kind in kinds], 'Document edits did not persist during the outage'
                queues[actor] += len(queued)
                receipts = [row for row in events if row['event'] == 'document_receipt']
                assert len(receipts) == config['stress_operations'] * len(kinds) and all(row['at_us'] > up for row in receipts)
    trace = [json.loads(line) for line in (output / 'smb-trace.jsonl').read_text().splitlines()]
    controls = [row['control'] for row in trace if row.get('control', {}).get('phase', '').startswith('offline-')]
    assert controls == [{'phase': 'offline-down', 'mode': 'down'}, {'phase': 'offline-reconnected'}], 'Unexpected outage control sequence'
    return {'outages': 1, 'confirmed_down_seconds': (up - down) / 1_000_000,
            'local_edits_while_down': queues, 'transport_connection_events': reconnects,
            'native_local_edits_inside_confirmed_outage': native_inside,
            'resumed_guarded_io_overlap': verify(trace, phase='offline-reconnected')}


def verify_lost_reply(output):
    from offline_history import publication_links, tokens
    config = json.loads((output / 'run.json').read_text())
    assert config['offline'] and config['offline_lost_reply'] and config['embedded_smb']
    assert config['stress_clients'] + config['rust_writers'] + config['rust_readers'] >= 12
    sample = json.loads((output / 'offline-lost-reply-progress.json').read_text())
    isolated = config.get('offline_client_reply', False)
    assert sample['up_started_us'] - sample['down_started_us'] >= 3_000_000
    actors = [*(f'w{i}' for i in range(config['rust_writers'])), *(f'r{i}' for i in range(config['rust_readers']))]
    assert set(sample['before']) == set(sample['after']) == set(actors)
    assert all(sample['after'][actor] >= sample['before'][actor] + 3 for actor in actors)
    if not config.get('document_operations'):
        assert all(sample['before'][actor] >= 3 for actor in actors)
    logs = {actor: [json.loads(line) for line in (output / 'rust' / f'{actor}.jsonl').read_text().splitlines()] for actor in actors}
    publication_links(logs, config['stress_operations'])
    documents = {}
    if config.get('document_operations'):
        from offline_document_history import document_history
        documents = document_history(logs, config['stress_operations'])
    for actor, rows in logs.items():
        if isolated and actor != 'w0': continue
        assert any(row['event'] == 'transport_connected' and row['at_us'] > sample['up_started_us'] for row in rows), f'{actor} did not reconnect'
    unknown = [(actor, row) for actor, rows in logs.items() for row in rows if row['event'] == 'remote_attempt' and row['state'] == 'Unknown']
    assert len(unknown) == 1, 'The reply cut did not establish exactly one uncertain publication'
    actor, attempt = unknown[0]
    assert attempt['started_us'] < sample['down_started_us'], 'Uncertain attempt started after the reply cut'
    receipts = [row for row in logs[actor] if row['event'] in ('remote_receipt', 'document_receipt') and row['revision'] == attempt['revision']]
    if not receipts and documents:
        target, = attempt['document_changes']
        confirmed_revision = documents[target]['states']['format']['attempt']['receipt_revision']
        receipts = [row for row in logs[actor] if row['event'] == 'document_receipt' and row['revision'] == confirmed_revision]
    assert len(receipts) == 1 and receipts[0]['at_us'] > sample['up_started_us']
    peer_progress = {}
    if isolated:
        assert sample['during']['w0'] == sample['before']['w0']
        retired, = [row for row in logs['r0'] if row['event'] == 'revision_retired']
        assert retired['space'] == attempt['space'] and retired['revision'] == attempt['revision']
        assert sample['down_started_us'] < retired['started_us'] <= retired['finished_us'] < sample['up_started_us']
        assert receipts[0]['revision'] != attempt['revision'], 'The client-disconnect gate did not exercise confirmation after revision retirement'
        paused, = [row for row in logs['w0'] if row['event'] == 'confirmation_paused']
        assert paused['revision'] == attempt['revision'] and attempt['finished_us'] <= paused['at_us'] < sample['up_started_us']
        for peer, rows in logs.items():
            if peer == 'w0': continue
            assert sample['during'][peer] >= sample['before'][peer] + 3
            if peer.startswith('w'):
                progress = [row for row in rows if row['event'] == 'remote_attempt' and row['state'] == 'Committed'
                            and sample['down_started_us'] < row['started_us'] <= row['finished_us'] < sample['up_started_us']]
            else:
                progress = [row for row in rows if row['event'] == 'read'
                            and sample['down_started_us'] < row['started_us'] <= row['finished_us'] < sample['up_started_us']]
            assert len(progress) >= 3, f'{peer} has insufficient completed I/O while the writer was disconnected'
            peer_progress[peer] = len(progress)
    if config.get('document_operations'):
        assert actor == 'w0' and sample['format_released_us'] <= attempt['started_us']
        intent, = [row for row in logs[actor] if row['event'] == 'local_document_commit' and row['id'] == receipts[0]['id']]
        assert intent['kind'] == 'format', 'The interrupted publication was not formatting'
        for writer in (f'w{i}' for i in range(config['rust_writers'])):
            paused = [row for row in logs[writer] if row['event'] == 'publication_paused']
            assert len(paused) == 1 and paused[0]['kind'] == 'format' and paused[0]['at_us'] < sample['format_released_us']
        paused, = [row for row in logs[actor] if row['event'] == 'publication_paused']
        if paused['revision'] != attempt['revision']:
            prior, = [row for row in logs[actor] if row['event'] == 'remote_attempt' and row['revision'] == paused['revision']]
            assert prior['state'] == 'NotCommitted' and sample['format_released_us'] <= prior['started_us'] <= prior['finished_us'] <= attempt['started_us'], 'Paused revision was replaced without proving it unpublished'
    captures = {}
    for rows in logs.values():
        for row in rows:
            if row['event'] != 'remote_confirm': continue
            name = row['capture']
            assert Path(name).name == name and name not in captures
            tokens(row['text'])
            assert row['started_us'] > sample['up_started_us']
            captures[name] = {'sha256': hashlib.sha256((output / 'rust/confirmations' / name).read_bytes()).hexdigest(), 'state': row['state']}
    assert captures and set(captures) == {path.name for path in (output / 'rust/confirmations').glob('*.one')}
    trace = [json.loads(line) for line in (output / 'smb-trace.jsonl').read_text().splitlines()]
    controls = [row['control'] for row in trace if row.get('control', {}).get('phase', '').startswith('offline-')]
    expected = {'phase': 'offline-reply-cut', 'cut': 9, 'peer': '10.0.2.2', 'offset': 96, 'direction': 'response'}
    if isolated: expected['scope'] = 'connection'
    assert controls == [expected, {'phase': 'offline-reply-reconnected'}]
    cuts = [row['cut'] for row in trace if 'cut' in row]
    assert len(cuts) == 1 and cuts[0]['direction'] == 'response' and cuts[0]['command'] == 9 and cuts[0]['status'] == '0x0'
    request, = [row for row in trace if row.get('direction') == 'request' and row['connection'] == cuts[0]['connection'] and row['message'] == cuts[0]['message']]
    assert request['offset'] == 96 and request['command'] == 9
    native_writes = 0
    if isolated:
        peers = {row['connection']: row['peer'][0] for row in trace if row.get('opened')}
        pending, files = {}, {}
        for row in trace:
            if row.get('command') not in (5, 6, 9): continue
            key = row['connection'], row['message']
            if row['direction'] == 'request':
                pending[key] = row
                if row['command'] == 6: files.pop((key[0], row['file_id']), None)
            elif row['status'] == '0x0' and key in pending:
                request = pending.pop(key)
                if row['command'] == 5:
                    files[key[0], row['file_id']] = request['path'].lower()
                elif (row['command'] == 9 and peers[key[0]].startswith('192.168.77.') and row.get('written', 0) > 0
                        and files.get((key[0], request['file_id']), '').endswith('synthetic.one')
                        and sample['down_started_us'] < request['time'] * 1_000_000 <= row['time'] * 1_000_000 < sample['up_started_us']):
                    native_writes += 1
        assert native_writes, 'No successful native writes while the isolated writer awaited reconciliation'
    return {'reply_cuts': 1, 'uncertain_actor': actor, 'attempted_revision': attempt['revision'], 'confirmed_revision': receipts[0]['revision'],
            'peer_operations_during_client_disconnect': peer_progress,
            'native_writes_during_client_disconnect': native_writes,
            'retired_snapshot_sha256': hashlib.sha256((output / 'rust/offline-retired.one').read_bytes()).hexdigest() if isolated else None,
            'confirmation_snapshots': captures, 'native_local_edits_inside_confirmed_outage': native_progress(output, config, sample),
            'resumed_guarded_io_overlap': verify(trace, phase='offline-reply-reconnected')}
