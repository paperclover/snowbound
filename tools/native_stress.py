"""Overlapping native and Rust editing histories on one shared section."""
import json
import os
import time

from native_runner import windows
from concurrent_rust import running_clients
from document_model import ordered_pages, walk


def edit_history(logs, operations, edit=False, *, partial=False, offline=False):
    if offline:
        assert not edit, 'Offline acceptance currently uses append intents'
        from offline_history import publication_links
        links = publication_links(logs, operations, partial)
    else:
        links = {}
        for actor, events in logs.items():
            assert events and events[0]['event'] == 'ready', 'Missing client start'
            assert partial or events[-1]['event'] == 'done', 'Incomplete client log'
            if not actor.startswith('w'): continue
            intents = {event['attempt']: event for event in events if event['event'] == 'intent'}
            commits = [event for event in events if event['event'] == 'commit']
            assert len(commits) <= operations, 'Unexpected Rust acknowledgement'
            expected = len(commits) if partial else operations
            assert sorted(event['operation'] for event in commits) == list(range(expected)), 'Missing or duplicate Rust acknowledgements'
            for event in commits:
                intent = intents[event['attempt']]
                token = f' [{actor}:{event["operation"]}]'
                offset = len(intent['before'].encode('utf-16-le')) // 2
                assert event['token'] == intent['token'] == token, 'Acknowledgement differs from intended edit'
                assert intent['replacement'] == (' café 🦀' if edit else '') + token, 'Unexpected replacement text'
                assert intent['operation'] == event['operation'] and intent['source_transaction'] == event['source_transaction'], 'Acknowledgement used another intent'
                start, end = intent['range']
                assert len('Concurrent edits:') <= start <= end <= offset, 'Invalid edit range'
                if not edit: assert start == end == offset, 'Expected an append intent'
                units = intent['before'].encode('utf-16-le')
                after = units[:start * 2].decode('utf-16-le') + intent['replacement'] + units[end * 2:].decode('utf-16-le')
                assert intent['before'] not in links, 'Acknowledged Rust edits branched from the same content'
                links[intent['before']] = event, after
    text = 'Concurrent edits:'
    versions = {text: 0}
    ordered = []
    while text in links:
        event, text = links.pop(text)
        ordered.append(event)
        versions[text] = len(ordered)
    assert not links, 'Acknowledged edits do not form one complete content history'
    for events in logs.values():
        previous = 0
        for event in events:
            if event['event'] != 'read': continue
            assert event['text'] in versions, 'A reader observed partial, lost or invented content'
            version = versions[event['text']]
            assert version >= previous, 'A reader went backwards in content history'
            previous = version
            for i, commit in enumerate(ordered):
                if commit['finished_us'] < event['started_us']:
                    assert version >= i + 1, 'A read missed an acknowledged edit'
                if commit['started_us'] > event['finished_us']:
                    assert version <= i, 'A read observed a future edit'
    return ordered, text


def native_history(events, actor, operations, edit):
    assert len(events) == operations, 'Missing native acknowledgements'
    prefix = f'Native {actor}:'
    expected = prefix
    for j, event in enumerate(events):
        assert event['operation'] == j and event['token'] == f' [n{actor}:{j}]' and event['before'] == expected, 'Native history lost or invented an edit'
        if edit:
            start, end = event['range']
            units = expected.encode('utf-16-le')
            assert len(prefix) <= start <= end <= len(units) // 2, 'Invalid native edit range'
            assert event['replacement'] == ' café 🦀' + event['token'], 'Unexpected native replacement'
            expected = units[:start * 2].decode('utf-16-le') + event['replacement'] + units[end * 2:].decode('utf-16-le')
        else:
            expected += event['token']
    return expected


def verify_capture(output, capture):
    import xml.etree.ElementTree as ET
    from native_format import native_characters
    from native_xml import ns

    config = json.loads((output / 'run.json').read_text())
    operations, edit = config['stress_operations'], config['edit']
    actors = [f'w{i}' for i in range(config['rust_writers'])] + [f'r{i}' for i in range(config['rust_readers'])]
    logs = {actor: [json.loads(line) for line in (output / 'rust' / f'{actor}.jsonl').read_text().splitlines()] for actor in actors}
    commits, rust_text = edit_history(logs, operations, edit, offline=config.get('offline', False))
    native = [[json.loads(line) for line in (output / f'n{i}' / 'stress-events.jsonl').read_text(encoding='utf-8-sig').splitlines()]
              for i in range(config['stress_clients'])]
    expected = [rust_text, *(native_history(events, i, operations, edit) for i, events in enumerate(native))]
    documents = {}
    if config.get('document_operations'):
        from offline_document_history import document_history
        documents = document_history(logs, operations)
        expected.extend(''.join(char for char, *_ in list(document['states'].values())[-1]['characters']) for document in documents.values())
    page_file, = capture.glob('page-*.xml')
    page = ET.parse(page_file).getroot()
    paragraphs = native_characters(page, page.findall('one:Outline', ns))
    actual = [''.join(char for char, _ in paragraph) for paragraph in paragraphs]
    assert sorted(actual) == sorted(expected), 'Fresh native content differs from the complete recorded editing history'
    checks = 0
    if edit:
        for text, events in zip(expected[1:], native, strict=True):
            for _, style in paragraphs[actual.index(text)]:
                for field in ('bold', 'italic'):
                    assert bool(style.get(field)) == events[-1][field], 'Fresh native formatting differs from its last recorded editing intent'
                    checks += 1
    if documents:
        from offline_document_history import verify_native
        checks += verify_native(paragraphs, (list(document['states'].values())[-1]['characters'] for document in documents.values()))
    return {'rust_intents': len(commits), 'document_intents': sum(len(document['states']) for document in documents.values()), 'native_intents': sum(map(len, native)),
            'exact_paragraphs': len(expected), 'native_intended_format_checks': checks}


def exercise(output, shared, clients, action, wait_action, wait_text, checkpoint, operations, sync_every, rust_writers, rust_readers, edit=False, seed=710, embedded_smb=False):
    wait_text(clients[0], ['Concurrent edits:'])
    action(clients[0], 'prepare-stress', clients=len(clients))
    prefixes = [f'Native {i}:' for i in range(len(clients))]
    for client in clients:
        wait_text(client, ['Concurrent edits:', *prefixes])
    checkpoint('stress-initial')
    config = json.loads((output / 'run.json').read_text())
    maintenance = config.get('maintenance', False)
    disconnect = config.get('disconnect', False)
    offline_outage = config.get('offline_outage', False)
    offline_lost_reply = config.get('offline_lost_reply', False)
    if maintenance:
        (output / 'maintenance-ready').touch()
        deadline = time.monotonic() + 300
        while not (output / 'maintenance-start').exists():
            if time.monotonic() > deadline: raise TimeoutError('Maintenance controller did not start the workload')
            time.sleep(.1)
    clocks = []
    for client in clients:
        before = time.time_ns() // 1000
        result = windows.do_health(client['name'])
        after = time.time_ns() // 1000
        native = result['utc_us']
        clocks.append({'native_minus_host_us': [native - after - 15625, native - before + 15625]})
    (output / 'clocks.json').write_text(json.dumps(clocks, indent=2))
    sequences = [action(client, 'stress', wait=False, actor=i, prefix=prefixes[i], operations=operations, seed=seed + 10000 + i, sync_every=sync_every, edit=edit, maintenance=maintenance)
                 for i, client in enumerate(clients)]
    for client, sequence in zip(clients, sequences):
        deadline = time.monotonic() + 60
        while time.monotonic() < deadline:
            result = windows.do_cmd(f'if exist C:\\one-tests\\runs\\capture\\outbox\\{sequence}\\ready echo ready', target=client['name'])
            if 'ready' in result.get('stdout', ''): break
            time.sleep(.1)
        else: raise TimeoutError('Native stress client did not reach the barrier')
    folder = output / 'rust'
    folder.mkdir()
    start = folder / 'start'
    from concurrent_rust import ROOT
    binaries = ROOT / 'target' / config.get('client_profile', 'debug') / 'examples'
    source = 'm6-collaboration\\synthetic.one' if embedded_smb else shared / 'synthetic.one'
    executable = binaries / ('smb_concurrent_client' if embedded_smb else 'concurrent_client')
    if disconnect: executable = binaries / 'smb_reconnect_client'
    if config.get('offline'): executable = binaries / 'smb_offline_client'
    environment = {**os.environ, 'ONESTORE_MAINTENANCE_DIR': str(folder)} if maintenance else None
    reader_executable = None
    if offline_outage:
        environment = {**os.environ, 'ONESTORE_OFFLINE_OUTAGE_DIR': str(folder)}
        reader_executable = binaries / 'smb_reconnect_client'
    if offline_lost_reply:
        captures = folder / 'confirmations'
        captures.mkdir()
        environment = {**os.environ, 'ONESTORE_OFFLINE_CONFIRM_DIR': str(captures)}
        reader_executable = binaries / 'smb_reconnect_client'
    if config.get('document_operations'):
        environment = {**(environment or os.environ), 'ONESTORE_OFFLINE_DOCUMENTS': '1'}
        if offline_lost_reply:
            environment['ONESTORE_OFFLINE_FORMAT_REPLY_DIR'] = str(folder)
    try:
        with running_clients(folder, source, rust_writers, rust_readers, operations, seed, timeout=config.get('client_timeout', 600), edit=edit, executable=executable, environment=environment, reader_executable=reader_executable) as processes:
            (shared / 'stress-start').write_text('start')
            for client, sequence in zip(clients, sequences):
                deadline = time.monotonic() + 60
                while time.monotonic() < deadline:
                    result = windows.do_cmd(f'if exist C:\\one-tests\\runs\\capture\\outbox\\{sequence}\\editing echo editing', target=client['name'])
                    if 'editing' in result.get('stdout', ''): break
                    time.sleep(.1)
                else: raise TimeoutError('Native stress client did not acknowledge its first edit')
            start.touch()
            if disconnect:
                from native_disconnect import interrupt
                interrupt(output, clients, sequences, processes)
            if offline_outage or offline_lost_reply:
                from offline_outage import interrupt
                interrupt(output, clients, sequences, processes)
            if embedded_smb:
                import linux_vm
                server = json.loads((output / 'run.json').read_text())['server']
                with (output / 'server-locks.jsonl').open('w') as trace:
                    for _ in range(100):
                        captured = linux_vm.run_ssh(server, 'sudo smbstatus --byterange --json', timeout=5)
                        captured.check_returncode()
                        trace.write(json.dumps(json.loads(captured.stdout)) + '\n')
                        trace.flush()
                        time.sleep(.1)
            for client, sequence in zip(clients, sequences):
                wait_action(client, sequence, 'stress')
    finally:
        for client, sequence in zip(clients, sequences):
            local = client['folder'] / 'stress-events.jsonl'
            try:
                result = windows.do_get(f'C:\\one-tests\\runs\\capture\\outbox\\{sequence}\\events.jsonl', local, client['name'])
            except Exception as error:
                result = {'error': str(error)}
            (client['folder'] / 'stress-capture.json').write_text(json.dumps(result, indent=2))
    for client, clock in zip(clients, clocks):
        before = time.time_ns() // 1000
        native = windows.do_health(client['name'])['utc_us']
        after = time.time_ns() // 1000
        clock['after_native_minus_host_us'] = [native - after - 15625, native - before + 15625]
    (output / 'clocks.json').write_text(json.dumps(clocks, indent=2))
    rust = {actor: [json.loads(line) for line in (folder / f'{actor}.jsonl').read_text().splitlines()] for actor in processes}
    commits, expected_rust = edit_history(rust, operations, edit, offline=config.get('offline', False))
    assert len(commits) == rust_writers * operations, 'Missing Rust acknowledgements'
    native_events = []
    for client in clients:
        local = client['folder'] / 'stress-events.jsonl'
        events = [json.loads(line) for line in local.read_text(encoding='utf-8-sig').splitlines()]
        native_events.append(events)
    prefixes = [native_history(events, i, operations, edit) for i, events in enumerate(native_events)]
    expected = [expected_rust, *prefixes]
    documents = {}
    if config.get('document_operations'):
        from offline_document_history import document_history
        documents = document_history(rust, operations)
        expected.extend(''.join(char for char, *_ in list(document['states'].values())[-1]['characters']) for document in documents.values())
    for client in clients: wait_text(client, expected)
    checkpoint('stress-final')
    model = json.loads((output / 'stress-final/model/document.json').read_text())
    for _, _, revision, _ in ordered_pages(model):
        assert not revision['nodes'][revision['roots']['1']]['spaces'], 'Disjoint edits created conflict pages'
    paragraphs = [n['kind']['text'] for _, _, revision, page in ordered_pages(model)
                  for _, n in walk(revision, page) if n['kind']['type'] == 'RichText']
    assert sorted(paragraphs) == sorted(expected), 'Final shared state lost, duplicated or added unrecorded content'
    if documents:
        from offline_document_history import verify_model
        verify_model(model, documents)
    if edit:
        resolved = json.loads((output / 'stress-final/model/text.json').read_text())
        for i, events in enumerate(native_events):
            runs, = [resolved[sid][rid][oid] for sid, rid, revision, page in ordered_pages(model)
                     for oid, node in walk(revision, page)
                     if node['kind']['type'] == 'RichText' and node['kind']['text'] == prefixes[i]]
            for run in runs:
                for field in ('bold', 'italic'):
                    assert bool(run['format'][field]) == events[-1][field], 'Final native formatting differs from its last edit'
    reads = [e for events in rust.values() for e in events if e['event'] == 'read']
    overlap = 0
    for events, clock in zip(native_events, clocks):
        low = min(clock['native_minus_host_us'][0], clock['after_native_minus_host_us'][0])
        high = max(clock['native_minus_host_us'][1], clock['after_native_minus_host_us'][1])
        for native in events:
            latest_start = (native['update_started_ticks'] - 621355968000000000) // 10 - low
            earliest_end = (native['updated_ticks'] - 621355968000000000) // 10 - high
            overlap += sum(max(latest_start, rust['started_us']) < min(earliest_end, rust['finished_us']) for rust in commits)
    result = {'native_writers': len(clients), 'rust_writers': rust_writers, 'rust_readers': rust_readers,
              'sync_every': sync_every, 'edit': edit, 'seed': seed, 'offline': config.get('offline', False), 'native_edits': operations * len(clients), 'rust_commits': len(commits), 'document_commits': sum(len(document['states']) for document in documents.values()), 'rust_reads': len(reads),
              'clock_bounded_native_rust_call_overlaps': overlap, 'converged_paragraphs': len(expected)}
    (output / 'result.json').write_text(json.dumps(result, indent=2))
    assert overlap, 'No native/Rust call overlap established within clock uncertainty'
    print(json.dumps(result), flush=True)


if __name__ == '__main__':
    import argparse
    from pathlib import Path

    parser = argparse.ArgumentParser(description='Check a fresh native capture against concurrent editing intents.')
    parser.add_argument('run', type=Path)
    parser.add_argument('capture', type=Path)
    args = parser.parse_args()
    print(json.dumps(verify_capture(args.run, args.capture), indent=2))
