import unittest

from native_maintenance import maintenance_locks


class Maintenance(unittest.TestCase):
    def history(self, path='m6-collaboration/synthetic.one', host='192.168.77.2', status='0xc0000055'):
        events = [{'connection': 1, 'opened': True, 'peer': [host, 1]}]
        def exchange(command, status='0x0', **fields):
            message = len(events)
            events.extend([
                {'connection': 1, 'direction': 'request', 'command': command, 'message': message, **fields},
                {'connection': 1, 'direction': 'response', 'command': command, 'message': message, 'status': status, 'file_id': 'file'},
            ])
        exchange(5, path=path)
        for phase, result in [('maintenance-held', status), ('maintenance-released', '0x0')]:
            events.append({'control': {'phase': phase}})
            exchange(10, result, file_id='file', locks=[(0xffffeffc, 4096, 0x12)])
        return events

    def test_section_denied_then_acquired(self):
        self.assertEqual(len(maintenance_locks(self.history())), 2)

    def test_toc_compaction_does_not_count(self):
        with self.assertRaises(AssertionError):
            maintenance_locks(self.history(path='m6-collaboration/Open Notebook.onetoc2'))

    def test_rust_maintenance_does_not_count(self):
        with self.assertRaises(AssertionError):
            maintenance_locks(self.history(host='10.0.2.2'))

    def test_unrelated_error_does_not_prove_exclusion(self):
        with self.assertRaises(AssertionError):
            maintenance_locks(self.history(status='0xc0000001'))

    def test_delayed_response_uses_request_phase(self):
        events = self.history()
        events.insert(-1, {'control': {'phase': 'maintenance-resumed'}})
        self.assertEqual(len(maintenance_locks(events)), 2)
        events = self.history()
        events.insert(-2, {'control': {'phase': 'maintenance-resumed'}})
        with self.assertRaises(AssertionError):
            maintenance_locks(events)

    def test_shared_or_short_lock_is_not_maintenance(self):
        for lock in [(0xffffeffc, 4096, 0x11), (0xfffffffc, 1, 0x12)]:
            events = self.history()
            for event in events:
                if 'locks' in event: event['locks'] = [lock]
            with self.assertRaises(AssertionError): maintenance_locks(events)


if __name__ == '__main__': unittest.main()
