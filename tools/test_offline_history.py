import copy
import unittest
from native_stress import edit_history


class OfflineHistoryTests(unittest.TestCase):
    def setUp(self):
        base = 'Concurrent edits:'
        self.logs = {}
        for actor, started, before in [('w1', 5, base), ('w0', 10, base + ' [w1:0]')]:
            token = f' [{actor}:0]'
            self.logs[actor] = [
                {'event': 'ready', 'offline': True},
                {'event': 'local_commit', 'id': 1, 'operation': 0, 'before': base, 'token': token, 'started_us': 1, 'finished_us': 2},
                {'event': 'read', 'text': before, 'started_us': started - 1, 'finished_us': started},
                {'event': 'remote_attempt', 'revision': actor, 'before': before, 'after': before + token, 'state': 'Committed', 'started_us': started, 'finished_us': started + 1},
                {'event': 'remote_receipt', 'id': 1, 'revision': actor, 'at_us': started + 2},
                {'event': 'reopened_receipt', 'id': 1, 'revision': actor},
                {'event': 'done'},
            ]
        self.logs['r0'] = [{'event': 'ready'}, {'event': 'read', 'text': base + ' [w1:0] [w0:0]', 'started_us': 14, 'finished_us': 15}, {'event': 'done'}]

    def test_private_local_branches_require_one_separate_remote_history(self):
        commits, text = edit_history(self.logs, 1, offline=True)
        self.assertEqual([event['token'] for event in commits], [' [w1:0]', ' [w0:0]'])
        self.assertEqual(text, self.logs['r0'][1]['text'])

    def test_false_receipts_lost_local_intents_and_changed_reopen_state_fail(self):
        for index, field, value in [(1, 'id', 2), (1, 'token', ' [w0:9]'), (3, 'state', 'Unknown'),
                                    (3, 'after', 'Concurrent edits: [w1:0]'), (4, 'at_us', 0),
                                    (5, 'revision', 'changed'), (3, 'revision', 'missing')]:
            with self.subTest(index=index, field=field):
                logs = copy.deepcopy(self.logs)
                logs['w0'][index][field] = value
                with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)
        for event in ('local_commit', 'remote_receipt', 'remote_attempt', 'reopened_receipt'):
            logs = copy.deepcopy(self.logs)
            logs['w0'] = [item for item in logs['w0'] if item['event'] != event]
            with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)

    def test_stale_reads_future_local_tokens_and_duplicate_acknowledgements_fail(self):
        logs = copy.deepcopy(self.logs)
        logs['r0'][1]['text'] = 'Concurrent edits:'
        with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)
        logs = copy.deepcopy(self.logs)
        logs['w0'][1]['before'] += ' [w2:0]'
        with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)
        for event in ('local_commit', 'remote_receipt', 'remote_attempt', 'reopened_receipt'):
            logs = copy.deepcopy(self.logs)
            copied = next(item for item in logs['w0'] if item['event'] == event)
            logs['w0'].insert(-1, copy.deepcopy(copied))
            with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)

    def test_uncertain_receipt_requires_the_original_target_revision_and_successful_confirmation(self):
        rows = self.logs['w0']
        intent = next(row for row in rows if row['event'] == 'local_commit')
        attempt = next(row for row in rows if row['event'] == 'remote_attempt')
        receipt = next(row for row in rows if row['event'] == 'remote_receipt')
        intent.update(space='page-space', object='paragraph')
        attempt.update(space='page-space', object='paragraph', state='Unknown')
        receipt['at_us'] = 14
        confirmation = dict(event='remote_confirm', state='Committed', started_us=12, finished_us=13,
                            revisions={'page-space': [attempt['revision']]}, text=attempt['after'])
        rows.insert(4, confirmation)
        self.assertEqual(len(edit_history(self.logs, 1, offline=True)[0]), 2)
        original = copy.deepcopy(self.logs)
        for field, value in [('state', 'NotCommitted'), ('state', 'Unknown'), ('finished_us', 15),
                             ('revisions', {'other-space': ['w0']}), ('revisions', {'page-space': ['wrong']}), ('text', 'Concurrent edits:')]:
            self.logs = copy.deepcopy(original)
            next(row for row in self.logs['w0'] if row['event'] == 'remote_confirm')[field] = value
            with self.subTest(field=field, value=value), self.assertRaises(AssertionError): edit_history(self.logs, 1, offline=True)
        self.logs = copy.deepcopy(original)
        replay = dict(attempt, revision='another-attempt', state='NotCommitted', started_us=11, finished_us=12)
        self.logs['w0'].insert(4, replay)
        with self.assertRaisesRegex(AssertionError, 'replayed'): edit_history(self.logs, 1, offline=True)

    def test_partial_capture_does_not_promote_pending_local_success(self):
        logs = copy.deepcopy(self.logs)
        logs['w0'] = logs['w0'][:2]
        logs['r0'] = [{'event': 'ready'}]
        commits, text = edit_history(logs, 1, offline=True, partial=True)
        self.assertEqual(len(commits), 1)
        self.assertEqual(text, 'Concurrent edits: [w1:0]')
        with self.assertRaises(AssertionError): edit_history(logs, 1, offline=True)


class OfflineLedgerTests(unittest.TestCase):
    def test_independent_ledger_queue_depth_and_progress_checks(self):
        import json
        import os
        from pathlib import Path
        import sqlite3
        import tempfile
        from verify_offline import verify
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder)
            (output / 'rust').mkdir()
            (output / 'run.json').write_text(json.dumps({'offline': True, 'embedded_smb': True, 'edit': False, 'stress_clients': 4, 'rust_writers': 4, 'rust_readers': 4, 'stress_operations': 2}))
            (output / 'rust/stop').touch()
            os.utime(output / 'rust/stop', ns=(0, 0))
            (output / 'rust/start').touch()
            os.utime(output / 'rust/start', ns=(0, 0))
            logs = {f'w{i}': [{'event': 'ready', 'offline': True}] for i in range(4)}
            for actor, events in logs.items():
                before = 'Concurrent edits:'
                for op in range(2):
                    token = f' [{actor}:{op}]'
                    events.append({'event': 'local_commit', 'id': op+1, 'operation': op, 'token': token, 'before': before, 'started_us': 1+op*2, 'finished_us': 2+op*2})
                    before += token
            before = 'Concurrent edits:'
            timestamp = 100
            for op in range(2):
                for actor, events in logs.items():
                    token, revision = f' [{actor}:{op}]', f'{actor}-{op}'
                    events.append({'event': 'remote_attempt', 'before': before, 'after': before+token, 'revision': revision, 'state': 'Committed', 'started_us': timestamp, 'finished_us': timestamp+1})
                    events.append({'event': 'remote_receipt', 'id': op+1, 'revision': revision, 'at_us': timestamp+2})
                    before += token
                    timestamp += 10
            for actor, events in logs.items():
                events.extend({'event': 'reopened_receipt', 'id': op+1, 'revision': f'{actor}-{op}'} for op in range(2))
                events.append({'event': 'done'})
                connection = sqlite3.connect(output / 'rust' / f'{actor}.sqlite')
                connection.executescript('CREATE TABLE receipts(edit_id INTEGER, revision TEXT); CREATE TABLE edits(id INTEGER); CREATE TABLE attempt(id INTEGER); CREATE TABLE conflicts(id INTEGER); CREATE TABLE replica(id INTEGER, base BLOB, working BLOB);')
                connection.executemany('INSERT INTO receipts VALUES (?,?)', [(op+1, f'{actor}-{op}') for op in range(2)])
                connection.execute('INSERT INTO replica VALUES (1, ?, ?)', (b'opaque image', b'opaque image'))
                connection.commit()
                connection.close()
            for i in range(4):
                logs[f'r{i}'] = [{'event': 'ready'}, {'event': 'read', 'text': before, 'started_us': timestamp, 'finished_us': timestamp+1}, {'event': 'done'}]
                (output / f'n{i}').mkdir()
                prefix = f'Native {i}:'
                native = []
                for op in range(2):
                    token = f' [n{i}:{op}]'
                    native.append({'operation': op, 'token': token, 'before': prefix, 'updated_ticks': op*10_000_000})
                    prefix += token
                (output / f'n{i}/stress-events.jsonl').write_text('\n'.join(json.dumps(event) for event in native))
            for actor, events in logs.items():
                (output / 'rust' / f'{actor}.jsonl').write_text('\n'.join(json.dumps(event) for event in events))
            result = verify(output, max_gap=2)
            self.assertEqual(result['remote_publications'], 8)
            self.assertEqual(result['queued_before_publication_lower_bound'], {f'w{i}': 2 for i in range(4)})
            with self.assertRaisesRegex(AssertionError, 'progress exceeded'): verify(output, max_gap=.5)
            os.utime(output / 'rust/stop', ns=(3_000_000_000, 3_000_000_000))
            with self.assertRaisesRegex(AssertionError, 'progress exceeded'): verify(output, max_gap=2)
            os.utime(output / 'rust/stop', ns=(0, 0))
            connection = sqlite3.connect(output / 'rust/w0.sqlite')
            for sql, undo in [("UPDATE receipts SET revision='wrong' WHERE edit_id=1", "UPDATE receipts SET revision='w0-0' WHERE edit_id=1"),
                              ('INSERT INTO attempt VALUES (1)', 'DELETE FROM attempt'),
                              ("UPDATE replica SET working=X'00'", "UPDATE replica SET working=base")]:
                connection.execute(sql)
                connection.commit()
                with self.assertRaises(AssertionError): verify(output)
                connection.execute(undo)
                connection.commit()
            connection.close()
