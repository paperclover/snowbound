import copy
import unittest

from crash_recovery import verify_text


class RecoveryOracleTest(unittest.TestCase):
    def test_accounting_and_corruption_controls(self):
        logs = {'w0': [
            {'event': 'intent', 'token': ' [w0:0]', 'attempt': 1},
            {'event': 'commit', 'token': ' [w0:0]', 'attempt': 1, 'started_us': 5, 'finished_us': 10},
            {'event': 'intent', 'token': ' [w0:1]', 'attempt': 2},
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
        with self.assertRaisesRegex(AssertionError, 'definitively uncommitted'):
            verify_text('Base', 'Base [w0:0] [w0:1]', rejected)
        future = copy.deepcopy(logs)
        future['w0'][1]['started_us'] = 18
        with self.assertRaisesRegex(AssertionError, 'future edit'):
            verify_text('Base', 'Base [w0:0]', future)
