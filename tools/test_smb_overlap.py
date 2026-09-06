import unittest

from verify_smb_overlap import verify


class Overlap(unittest.TestCase):
    def history(self, serialized=False, failed_write=False, writer_reads=False):
        events = [{'connection': 1, 'opened': True, 'peer': ['10.0.2.2', 1]},
                  {'connection': 2, 'opened': True, 'peer': ['192.168.77.2', 2]}]
        def exchange(connection, command, status='0x0', **fields):
            message = len(events)
            request = {'connection': connection, 'command': command, 'message': message,
                       'direction': 'request', **fields}
            response = {'connection': connection, 'command': command, 'message': message,
                        'direction': 'response', 'status': status, 'file_id': str(connection)}
            events.extend((request, response))
        for connection in (1, 2):
            exchange(connection, 5, path='synthetic.one', access=0xc0000000 if connection == 2 or writer_reads else 0x80000000)
            exchange(connection, 10, file_id=str(connection), locks=[(0xfffffffb, 1, 0x11)])
        if serialized:
            exchange(1, 8, file_id='1', length=32)
            exchange(1, 6, file_id='1')
        exchange(2, 10, file_id='2', locks=[(0xfffffffd, 1, 0x12)])
        if not serialized:
            exchange(1, 8, file_id='1', length=32)
        exchange(2, 9, status='0xc0000054' if failed_write else '0x0', file_id='2', length=32)
        exchange(2, 6, file_id='2')
        events.append({'connection': 3, 'opened': True, 'peer': ['10.0.2.2', 3]})
        exchange(3, 5, path='synthetic.one', access=0xc0000000)
        exchange(3, 10, file_id='3', locks=[(0xfffffffb, 1, 0x11), (0xfffffffd, 1, 0x12)])
        if not serialized:
            exchange(1, 8, file_id='1', length=32)
        exchange(3, 9, file_id='3', length=32)
        return events

    def test_active_read_and_native_write(self):
        result = verify(self.history())
        self.assertEqual(result['active_native_writer_pairs'], 1)
        self.assertEqual(result['active_rust_writer_pairs'], 1)
        self.assertEqual(result['overlapping_reads'], 2)
        self.assertEqual(result['overlapping_writes'], 2)

    def test_progress_before_resume_does_not_satisfy_after_gate(self):
        events = self.history()
        events.append({'control': {'phase': 'resumed'}})
        with self.assertRaises(AssertionError):
            verify(events, phase='resumed')
        with self.assertRaises(AssertionError):
            verify(self.history(), phase='resumed')
        self.assertEqual(verify([{'control': {'phase': 'resumed'}}, *self.history()], phase='resumed')['active_rust_writer_pairs'], 1)

    def test_native_only_traffic_does_not_count(self):
        events = self.history()
        for event in events:
            if event.get('opened'): event['peer'][0] = '192.168.77.' + str(event['connection'] + 1)
        with self.assertRaises(AssertionError):
            verify(events)

    def test_delayed_io_responses_do_not_count_as_resumed_requests(self):
        events = self.history()
        end = next(i for i, event in enumerate(events) if event.get('direction') == 'request' and event['command'] == 6)
        responses = [event for event in events[:end] if event.get('direction') == 'response' and event['command'] in (8, 9)]
        events = [event for event in events if event not in responses]
        events[end - len(responses):end - len(responses)] = [{'control': {'phase': 'resumed'}}, *responses]
        self.assertEqual(verify(events)['active_native_writer_pairs'], 1)
        with self.assertRaises(AssertionError):
            verify(events, phase='resumed')

    def test_serialized_calls_do_not_count(self):
        with self.assertRaises(AssertionError):
            verify(self.history(serialized=True))

    def test_failed_write_does_not_count(self):
        with self.assertRaises(AssertionError):
            verify(self.history(failed_write=True))

    def test_writers_validation_reads_do_not_count(self):
        with self.assertRaises(AssertionError):
            verify(self.history(writer_reads=True))

    def test_lock_failure_does_not_count(self):
        events = self.history()
        for event in events:
            if event.get('direction') == 'response' and event['command'] == 10:
                event['status'] = '0xc0000055'
        with self.assertRaises(AssertionError):
            verify(events)

    def test_separate_lock_lifetimes_do_not_combine(self):
        events = self.history()
        inserted = []
        for message, flags in [(1000, 4), (1001, 0x12)]:
            inserted.extend([
                {'connection': 3, 'direction': 'request', 'command': 10, 'message': message,
                 'file_id': '3', 'locks': [(0xfffffffd, 1, flags)]},
                {'connection': 3, 'direction': 'response', 'command': 10, 'message': message,
                 'status': '0x0'},
            ])
        events[-2:-2] = inserted
        with self.assertRaises(AssertionError):
            verify(events)

    def test_renamed_path_does_not_combine_file_versions(self):
        events = self.history()
        events[-2:-2] = [
            {'connection': 3, 'direction': 'request', 'command': 17, 'message': 1000,
             'info_type': 1, 'info_class': 10},
            {'connection': 3, 'direction': 'response', 'command': 17, 'message': 1000, 'status': '0x0'},
        ]
        with self.assertRaises(AssertionError):
            verify(events)

    def test_open_response_crossing_rename_is_not_attributed(self):
        events = self.history()
        at = next(i for i, event in enumerate(events) if event.get('connection') == 3
                  and event.get('command') == 5 and event['direction'] == 'response')
        inserted = [
            {'connection': 4, 'opened': True, 'peer': ['192.168.77.4', 4]},
            {'connection': 4, 'direction': 'request', 'command': 17, 'message': 1000,
             'info_type': 1, 'info_class': 10},
            {'connection': 4, 'direction': 'response', 'command': 17, 'message': 1000, 'status': '0x0'},
        ]
        for message, command, fields in [
            (1001, 5, {'path': 'synthetic.one', 'access': 0x80000000}),
            (1002, 10, {'file_id': '1', 'locks': [(0xfffffffb, 1, 0x11)]}),
        ]:
            inserted.extend([
                {'connection': 1, 'direction': 'request', 'command': command, 'message': message, **fields},
                {'connection': 1, 'direction': 'response', 'command': command, 'message': message,
                 'status': '0x0', 'file_id': '1'},
            ])
        events[at:at] = inserted
        with self.assertRaises(AssertionError):
            verify(events)

    def test_connection_close_ends_guard(self):
        events = self.history()
        events.insert(-2, {'connection': 1, 'closed': True})
        with self.assertRaises(AssertionError):
            verify(events)


if __name__ == '__main__':
    unittest.main()
