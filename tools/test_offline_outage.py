import copy
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from offline_outage import verify_outage, verify_lost_reply


class OutageOracle(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'rust').mkdir()
        self.config = dict(offline=True, offline_outage=True, embedded_smb=True, stress_clients=4, rust_writers=4, rust_readers=4, stress_operations=8)
        self.sample = dict(down_started_us=1_000_000, up_started_us=5_000_000, native_before=[1]*4, native_during=[4]*4, reader_errors_before={f'r{i}': 0 for i in range(4)})
        (self.root / 'clocks.json').write_text(json.dumps([dict(native_minus_host_us=[-10, 10], after_native_minus_host_us=[-10, 10])]*4))
        for i in range(4):
            (self.root / f'n{i}').mkdir()
            (self.root / f'n{i}/stress-events.jsonl').write_text(json.dumps(dict(update_started_ticks=621355968020000000, updated_ticks=621355968030000000)))
        self.logs = {}
        for i in range(4):
            self.logs[f'w{i}'] = [dict(event='ready'), dict(event='transport_connected', at_us=0),
                dict(event='publication_paused', revision=f'w{i}', at_us=100),
                *[dict(event='local_commit', operation=j, started_us=10 if j == 0 else 2_000_000+j, finished_us=20 if j == 0 else 2_000_010+j) for j in range(8)],
                dict(event='remote_attempt', started_us=6_000_000, state='NotCommitted', revision=f'w{i}'),
                dict(event='transport_connected', at_us=6_000_001),
                *[dict(event='remote_receipt', at_us=7_000_000+j) for j in range(8)], dict(event='done')]
            self.logs[f'r{i}'] = [dict(event='ready'), dict(event='transport_read_error'), dict(event='transport_connected', at_us=6_000_000),
                *[dict(event='read', started_us=7_000_000+j, finished_us=7_000_001+j) for j in range(3)], dict(event='done')]
        self.trace = [dict(control=dict(phase='offline-down', mode='down')), dict(control=dict(phase='offline-reconnected'))]

    def verify(self):
        (self.root / 'run.json').write_text(json.dumps(self.config))
        (self.root / 'offline-outage-progress.json').write_text(json.dumps(self.sample))
        (self.root / 'smb-trace.jsonl').write_text('\n'.join(map(json.dumps, self.trace)))
        for actor, events in self.logs.items():
            (self.root / 'rust' / f'{actor}.jsonl').write_text('\n'.join(map(json.dumps, events)))
        with patch('offline_outage.verify', return_value={'guarded_pairs': 1}) as overlap:
            result = verify_outage(self.root)
            overlap.assert_called_once_with(self.trace, phase='offline-reconnected')
            return result

    def test_confirmed_outage_requires_local_and_native_progress_and_fresh_sessions(self):
        self.assertEqual(self.verify()['local_edits_while_down'], {f'w{i}': 7 for i in range(4)})
        original = copy.deepcopy(self.logs)
        for event, field, value in [('local_commit', 'started_us', 0), ('publication_paused', 'at_us', 3_000_000),
                                    ('remote_attempt', 'state', 'Unknown'), ('remote_attempt', 'started_us', 3_000_000),
                                    ('remote_attempt', 'revision', 'other'), ('remote_receipt', 'at_us', 0)]:
            self.logs = copy.deepcopy(original)
            row = next(row for row in self.logs['w0'] if row['event'] == event and (event != 'local_commit' or row['operation'] == 1))
            row[field] = value
            with self.subTest(event=event, field=field), self.assertRaises(AssertionError): self.verify()
        for actor, event in [('r0', 'transport_connected'), ('r0', 'transport_read_error'), ('r0', 'read'), ('w0', 'publication_paused')]:
            self.logs = copy.deepcopy(original)
            self.logs[actor] = [row for row in self.logs[actor] if row['event'] != event]
            with self.subTest(actor=actor, event=event), self.assertRaises(AssertionError): self.verify()

    def test_lost_reply_requires_one_original_revision_receipt_and_captured_confirmation(self):
        self.config.update(offline_outage=False, offline_lost_reply=True)
        self.sample.update(before={actor: 3 for actor in self.logs}, after={actor: 6 for actor in self.logs})
        attempt = next(row for row in self.logs['w0'] if row['event'] == 'remote_attempt')
        attempt.update(state='Unknown', started_us=500_000, finished_us=1_100_000)
        receipt = next(row for row in self.logs['w0'] if row['event'] == 'remote_receipt')
        receipt['revision'] = attempt['revision']
        for row in self.logs['w0']:
            if row['event'] == 'remote_receipt' and row is not receipt: row['revision'] = 'other'
        folder = self.root / 'rust/confirmations'
        folder.mkdir()
        (folder / 'confirmation.one').write_bytes(b'owned captured image')
        confirmation = dict(event='remote_confirm', state='Committed', started_us=6_000_000, finished_us=6_000_001,
                            text='Concurrent edits: [w0:0]', capture='confirmation.one')
        self.logs['w0'].insert(-1, confirmation)
        self.trace = [dict(control=dict(phase='offline-reply-cut', cut=9, peer='10.0.2.2', offset=96, direction='response')),
                      dict(direction='request', command=9, offset=96, connection=1, message=2),
                      dict(cut=dict(direction='response', command=9, status='0x0', connection=1, message=2)),
                      dict(control=dict(phase='offline-reply-reconnected'))]
        def check():
            (self.root / 'run.json').write_text(json.dumps(self.config))
            (self.root / 'offline-lost-reply-progress.json').write_text(json.dumps(self.sample))
            (self.root / 'smb-trace.jsonl').write_text('\n'.join(map(json.dumps, self.trace)))
            for actor, rows in self.logs.items():
                (self.root / 'rust' / f'{actor}.jsonl').write_text('\n'.join(map(json.dumps, rows)))
            with patch('offline_history.publication_links') as ledger, patch('offline_document_history.document_history') as documents, patch('offline_outage.verify', return_value={'guarded_pairs': 1}) as overlap:
                if self.config.get('offline_client_reply'):
                    documents.return_value = {'target': {'format': {'receipt_revision': 'effect-revision'}}}
                result = verify_lost_reply(self.root)
                ledger.assert_called_once_with(self.logs, self.config['stress_operations'])
                if self.config.get('document_operations'):
                    documents.assert_called_once_with(self.logs, self.config['stress_operations'])
                else:
                    documents.assert_not_called()
                overlap.assert_called_once_with(self.trace, phase='offline-reply-reconnected')
                return result
        self.assertEqual(check()['confirmed_revision'], 'w0')
        self.config['document_operations'] = True
        self.sample['format_released_us'] = 200_000
        receipt['event'] = 'document_receipt'
        receipt['id'] = 17
        intent = dict(event='local_document_commit', id=17, kind='format')
        self.logs['w0'].insert(-1, intent)
        for actor, rows in self.logs.items():
            if actor.startswith('w'):
                next(row for row in rows if row['event'] == 'publication_paused')['kind'] = 'format'
        self.assertEqual(check()['confirmed_revision'], 'w0')
        intent['kind'] = 'insert'
        with self.assertRaisesRegex(AssertionError, 'not formatting'): check()
        intent['kind'] = 'format'
        self.sample['format_released_us'] = 900_000
        with self.assertRaises(AssertionError): check()
        self.sample['format_released_us'] = 200_000
        paused = next(row for row in self.logs['w0'] if row['event'] == 'publication_paused')
        paused['revision'] = 'stale-prepared-revision'
        prior = dict(event='remote_attempt', revision=paused['revision'], state='NotCommitted',
                     started_us=200_001, finished_us=200_002)
        self.logs['w0'].insert(-1, prior)
        self.assertEqual(check()['confirmed_revision'], 'w0')
        prior['finished_us'] = 600_000
        with self.assertRaisesRegex(AssertionError, 'proving it unpublished'): check()
        self.logs['w0'].remove(prior)
        paused['revision'] = attempt['revision']
        self.config['offline_client_reply'] = True
        receipt['revision'] = 'effect-revision'
        attempt.update(space='space', document_changes={'target': {}})
        retirement = dict(event='revision_retired', space='space', revision=attempt['revision'], started_us=2_200_000, finished_us=2_300_000)
        self.logs['r0'].append(retirement)
        (self.root / 'rust/offline-retired.one').write_bytes(b'retired revision snapshot')
        self.sample['during'] = {actor: 3 if actor == 'w0' else 6 for actor in self.logs}
        self.trace[0]['control']['scope'] = 'connection'
        barrier = dict(event='confirmation_paused', revision=attempt['revision'], at_us=1_200_000)
        self.logs['w0'].append(barrier)
        peer_rows = []
        for actor, rows in self.logs.items():
            if actor == 'w0': continue
            for i in range(3):
                row = dict(event='remote_attempt' if actor.startswith('w') else 'read', state='Committed',
                           started_us=2_000_000+i, finished_us=2_000_010+i)
                rows.append(row)
                peer_rows.append((rows, row))
        wire = [dict(connection=7, opened=True, peer=['192.168.77.12', 445]),
                dict(connection=7, message=2, command=5, direction='request', path='owned/synthetic.one'),
                dict(connection=7, message=2, command=5, direction='response', status='0x0', file_id='native-file'),
                dict(connection=7, message=3, command=9, direction='request', time=2.0, file_id='native-file'),
                dict(connection=7, message=3, command=9, direction='response', time=2.1, status='0x0', written=4)]
        self.trace.extend(wire)
        result = check()
        self.assertEqual(result['native_writes_during_client_disconnect'], 1)
        self.assertEqual(result['peer_operations_during_client_disconnect'], {actor: 3 for actor in self.logs if actor != 'w0'})
        peer_rows[0][1]['finished_us'] = 6_000_000
        with self.assertRaisesRegex(AssertionError, 'insufficient completed I/O'): check()
        peer_rows[0][1]['finished_us'] = 2_000_010
        wire[-1]['time'] = 6.0
        with self.assertRaisesRegex(AssertionError, 'No successful native writes'): check()
        wire[-1]['time'] = 2.1
        barrier['revision'] = 'other'
        with self.assertRaises(AssertionError): check()
        self.logs['w0'].remove(barrier)
        self.logs['r0'].remove(retirement)
        receipt['revision'] = attempt['revision']
        for rows, row in peer_rows: rows.remove(row)
        del self.trace[-len(wire):]
        del self.trace[0]['control']['scope']
        self.config['offline_client_reply'] = False
        self.config['document_operations'] = False
        receipt['event'] = 'remote_receipt'
        self.logs['w0'].remove(intent)
        attempt['state'] = 'Committed'
        with self.assertRaises(AssertionError): check()
        attempt['state'] = 'Unknown'
        self.trace[1]['offset'] = 100
        with self.assertRaises(AssertionError): check()
        self.trace[1]['offset'] = 96
        (folder / 'unrecorded.one').write_bytes(b'extra')
        with self.assertRaises(AssertionError): check()

    def test_document_outage_requires_both_local_operations_and_delayed_receipts(self):
        self.config['document_operations'] = True
        for actor, events in self.logs.items():
            if not actor.startswith('w'): continue
            events[-1:-1] = [dict(event='local_document_commit', operation=operation, kind=kind,
                                 started_us=10 if operation == 0 else 2_100_000+operation,
                                 finished_us=20 if operation == 0 else 2_100_010+operation)
                             for operation in range(8) for kind in ('insert', 'format')]
            events[-1:-1] = [dict(event='document_receipt', at_us=7_100_000+i) for i in range(16)]
        self.assertEqual(self.verify()['local_edits_while_down'], {f'w{i}': 21 for i in range(4)})
        original = copy.deepcopy(self.logs)
        for event, field, value in [('local_document_commit', 'finished_us', 6_000_000),
                                    ('local_document_commit', 'kind', 'insert'),
                                    ('document_receipt', 'at_us', 0)]:
            self.logs = copy.deepcopy(original)
            row = next(row for row in self.logs['w0'] if row['event'] == event
                       and (event != 'local_document_commit' or row['operation'] == 1 and row['kind'] == 'format'))
            row[field] = value
            with self.subTest(event=event, field=field), self.assertRaises(AssertionError): self.verify()

    def test_native_edits_outside_confirmed_window_fail(self):
        (self.root / 'n0/stress-events.jsonl').write_text(json.dumps(dict(update_started_ticks=621355968000000000, updated_ticks=621355968000000100)))
        with self.assertRaisesRegex(AssertionError, 'timestamps'): self.verify()

    def test_native_stall_short_outage_and_wrong_control_fail(self):
        self.sample['native_before'].append(1)
        with self.assertRaises(AssertionError): self.verify()
        self.sample['native_before'].pop()
        self.sample['native_during'][0] = 1
        with self.assertRaises(AssertionError): self.verify()
        self.sample['native_during'][0] = 4
        self.sample['up_started_us'] = 1_000_001
        with self.assertRaises(AssertionError): self.verify()
        self.sample['up_started_us'] = 5_000_000
        self.trace[0]['control'].pop('mode')
        with self.assertRaises(AssertionError): self.verify()


if __name__ == '__main__': unittest.main()
