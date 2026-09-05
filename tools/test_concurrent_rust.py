import copy
import unittest
from concurrent_rust import verify


class OracleTests(unittest.TestCase):
    def setUp(self):
        self.logs = {
            'w0': [{'event': 'ready'}, {'event': 'commit', 'source_transaction': 1,
                'operation': 0, 'token': ' [w0:0]', 'started_us': 10, 'finished_us': 30}, {'event': 'done'}],
            'w1': [{'event': 'ready'}, {'event': 'commit', 'source_transaction': 2,
                'operation': 0, 'token': ' [w1:0]', 'started_us': 20, 'finished_us': 40}, {'event': 'done'}],
            'r0': [{'event': 'ready'}, {'event': 'read', 'transaction': 3,
                'text': 'Concurrent edits: [w0:0] [w1:0]', 'started_us': 50, 'finished_us': 60}, {'event': 'done'}],
        }

    def test_valid_history_and_deliberate_corruption(self):
        self.assertEqual(verify(self.logs, 1, 2, 1)['commits'], 2)
        for field, value in [('text', 'Concurrent edits: [w1:0]'), ('transaction', 2), ('finished_us', 19)]:
            logs = copy.deepcopy(self.logs)
            logs['r0'][1][field] = value
            with self.assertRaises(AssertionError):
                verify(logs, 1, 2, 1)

    def test_duplicate_snapshot_and_missing_acknowledgement(self):
        logs = copy.deepcopy(self.logs)
        logs['w1'][1]['source_transaction'] = 1
        with self.assertRaises(AssertionError):
            verify(logs, 1, 2, 1)
        logs['w1'].pop(1)
        with self.assertRaises(AssertionError):
            verify(logs, 1, 2, 1)

    def test_length_changing_unicode_history_and_wrong_intent(self):
        previous = 'Concurrent edits:'
        for i in range(2):
            events = self.logs[f'w{i}']
            event = events[1]
            event['attempt'] = 1
            replacement = ' café 🦀' + event['token']
            events.insert(1, {'event': 'intent', 'attempt': 1, 'operation': 0,
                'source_transaction': i + 1, 'before': previous, 'token': event['token'],
                'range': [17, len(previous.encode('utf-16-le')) // 2], 'replacement': replacement})
            previous = 'Concurrent edits:' + replacement
        self.logs['r0'][1]['text'] = previous
        self.assertEqual(verify(self.logs, 1, 2, 1, edit=True)['final_text'], previous)
        self.logs['w1'][1]['range'][0] = 24
        with self.assertRaises(UnicodeDecodeError):
            verify(self.logs, 1, 2, 1, edit=True)

    def test_serial_calls_do_not_count_as_concurrency(self):
        self.logs['w1'][1]['started_us'] = 31
        with self.assertRaisesRegex(AssertionError, 'never overlapped'):
            verify(self.logs, 1, 2, 1)


if __name__ == '__main__':
    unittest.main()
