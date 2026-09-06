import copy
import json
from pathlib import Path
import subprocess
import unittest

from crash_recovery import active_text, verify_text
from document_model import EXPORTER, ordered_pages, view, walk


class RecoveryOracleTest(unittest.TestCase):
    def test_current_page_is_distinguished_from_native_conflict_content(self):
        source = Path(__file__).resolve().parent.parent / 'corpus/collaboration/round-01/offline/notebook/synthetic.one'
        model = json.loads(subprocess.check_output([EXPORTER, source]))
        (sid, _, revision, page), = ordered_pages(model)
        main = next(node for _, node in walk(revision, page) if node['kind']['type'] == 'RichText')
        main['kind']['text'] = 'Concurrent edits: current'
        manifest = revision['nodes'][revision['roots']['1']]
        self.assertTrue(manifest['spaces'])
        for conflict in manifest['spaces']:
            self.assertNotEqual(sid, conflict)
            _, older = view(model, conflict)
            node = next(node for node in older['nodes'].values() if node['kind']['type'] == 'RichText')
            node['kind']['text'] = 'Concurrent edits: older'
        self.assertEqual(active_text(model), 'Concurrent edits: current')

    def test_accounting_and_corruption_controls(self):
        logs = {'w0': [
            {'event': 'intent', 'token': ' [w0:0]', 'replacement': ' [w0:0]', 'before': 'Base', 'range': [4, 4], 'attempt': 1},
            {'event': 'commit', 'token': ' [w0:0]', 'attempt': 1, 'started_us': 5, 'finished_us': 10},
            {'event': 'intent', 'token': ' [w0:1]', 'replacement': ' [w0:1]', 'before': 'Base [w0:0]', 'range': [11, 11], 'attempt': 2},
            {'event': 'commit_error', 'token': ' [w0:1]', 'attempt': 2, 'state': 'Unknown', 'started_us': 18, 'finished_us': 20},
            {'event': 'read', 'text': 'Base [w0:0]', 'started_us': 15, 'finished_us': 17},
        ]}
        self.assertEqual(verify_text('Base', 'Base [w0:0] [w0:1]', logs)['unacknowledged_retained'], [' [w0:1]'])
        self.assertEqual(verify_text('Base', 'Base [w0:0]', logs)['acknowledged'], 1)
        for text in ['Base', 'Other [w0:0]', 'Base [w0:0] [w0:0]', 'Base [w0:0] [w0:2]', 'Base [w0:0] [w0:']:
            with self.assertRaises(AssertionError):
                verify_text('Base', text, logs)
        lost_read = copy.deepcopy(logs)
        lost_read['w0'][-1]['text'] = 'Base'
        with self.assertRaisesRegex(AssertionError, 'read missed'):
            verify_text('Base', 'Base [w0:0]', lost_read)
        committed_error = copy.deepcopy(logs)
        committed_error['w0'][3]['state'] = 'Committed'
        with self.assertRaisesRegex(AssertionError, 'acknowledged edit was lost'):
            verify_text('Base', 'Base [w0:0]', committed_error)
        rejected = copy.deepcopy(logs)
        rejected['w0'][3]['state'] = 'NotCommitted'
        with self.assertRaisesRegex(AssertionError, 'recorded publication'):
            verify_text('Base', 'Base [w0:0] [w0:1]', rejected)
        with self.assertRaisesRegex(AssertionError, 'recorded publication'):
            verify_text('Base', 'Base [w0:1] [w0:0]', logs)
        future = copy.deepcopy(logs)
        future['w0'][1]['started_us'] = 18
        with self.assertRaisesRegex(AssertionError, 'future edit'):
            verify_text('Base', 'Base [w0:0]', future)
