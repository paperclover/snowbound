#!/usr/bin/env python3
"""Audit completed offline/native runs, including the independent SQLite receipt ledger."""
import argparse
import hashlib
import json
from pathlib import Path
import sqlite3

from native_stress import edit_history, native_history


def verify(output, max_gap=120):
    config = json.loads((output / 'run.json').read_text())
    assert config['offline'] and config['embedded_smb'] and not config['edit']
    assert config['stress_clients'] + config['rust_writers'] + config['rust_readers'] >= 12
    actors = [*(f'w{i}' for i in range(config['rust_writers'])), *(f'r{i}' for i in range(config['rust_readers']))]
    logs = {actor: [json.loads(line) for line in (output / 'rust' / f'{actor}.jsonl').read_text().splitlines()] for actor in actors}
    commits, text = edit_history(logs, config['stress_operations'], offline=True)
    documents = {}
    if config.get('document_operations'):
        from offline_document_history import document_history
        documents = document_history(logs, config['stress_operations'])
    started = (output / 'rust/start').stat().st_mtime_ns // 1000
    stopped = (output / 'rust/stop').stat().st_mtime_ns // 1000
    progress, queues, caches = {}, {}, {}
    for actor, events in logs.items():
        times = [event['at_us'] for event in events if event['event'] in ('remote_receipt', 'document_receipt')] if actor.startswith('w') else [event['finished_us'] for event in events if event['event'] == 'read']
        assert times and times == sorted(times), 'Client progress is absent or went backwards'
        if actor.startswith('r'): times.append(max(times[-1], stopped))
        progress[actor] = max(b-a for a, b in zip([started, *times], times)) / 1_000_000
        if not actor.startswith('w'): continue
        edits = {event['id']: event for event in events if event['event'] in ('local_commit', 'local_document_commit')}
        receipts = {event['id']: event for event in events if event['event'] in ('remote_receipt', 'document_receipt')}
        attempts = {event['revision']: event for event in events if event['event'] == 'remote_attempt'}
        publication_starts = {id: documents[edit['object']][edit['kind']]['started_us'] if edit['event'] == 'local_document_commit'
                              else attempts[receipts[id]['revision']]['started_us'] for id, edit in edits.items()}
        queues[actor] = max(sum(edit['finished_us'] <= at < publication_starts[id] for id, edit in edits.items()) for at in [edit['finished_us'] for edit in edits.values()])
        assert queues[actor] >= 2, 'Writer did not establish a durable local queue before publication'
        path = output / 'rust' / f'{actor}.sqlite'
        connection = sqlite3.connect(path.resolve().as_uri() + '?mode=ro', uri=True)
        try:
            assert connection.execute('PRAGMA quick_check').fetchall() == [('ok',)], 'Cache integrity failed'
            assert connection.execute('PRAGMA foreign_key_check').fetchall() == [], 'Cache foreign keys failed'
            for table in ('edits', 'attempt', 'conflicts'):
                assert connection.execute(f'SELECT count(*) FROM {table}').fetchone() == (0,), 'Completed cache retains unresolved state'
            persisted = dict(connection.execute('SELECT edit_id, revision FROM receipts'))
            assert persisted == {id: event['revision'] for id, event in receipts.items()}, 'SQLite receipts differ from observed acknowledgements'
            base, working = connection.execute('SELECT base, working FROM replica WHERE id=1').fetchone()
            assert base == working, 'A drained cache retained a divergent local branch'
            caches[actor] = {'receipts': len(persisted), 'image_bytes': len(base), 'image_sha256': hashlib.sha256(base).hexdigest(), 'database_sha256': hashlib.sha256(path.read_bytes()).hexdigest()}
        finally:
            connection.close()
    for i in range(config['stress_clients']):
        events = [json.loads(line) for line in (output / f'n{i}/stress-events.jsonl').read_text(encoding='utf-8-sig').splitlines()]
        native_history(events, i, config['stress_operations'], False)
        times = [event['updated_ticks'] for event in events]
        assert times == sorted(times)
        progress[f'n{i}'] = max((b-a for a, b in zip(times, times[1:])), default=0) / 10_000_000
    assert all(gap <= max_gap for gap in progress.values()), f'Client progress exceeded {max_gap}s: {progress}'
    return {'remote_publications': len(commits), 'document_publications': len(documents)*2, 'remote_text_sha256': hashlib.sha256(text.encode()).hexdigest(), 'maximum_progress_gap_seconds': progress,
            'queued_before_publication_lower_bound': queues, 'reviewed_placements': sum(event['event'] == 'reviewed_append' for events in logs.values() for event in events), 'caches': caches}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--max-gap', type=float, default=120)
    args = parser.parse_args()
    result = verify(args.output, args.max_gap)
    (args.output / 'offline-verification.json').write_text(json.dumps(result, indent=2))
    print(json.dumps(result, indent=2))
