import copy
import json
import os
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from native_disconnect import interrupt, verify_disconnect


class DisconnectOracle(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        (self.root / 'rust').mkdir()
        self.config = {'stress_clients': 4, 'rust_writers': 4, 'rust_readers': 4, 'stress_operations': 80}
        for name, stamp in [('start', 0), ('stop', 10**9)]:
            path = self.root / 'rust' / name
            path.touch()
            os.utime(path, ns=(stamp, stamp))
        for i in range(4):
            folder = self.root / f'n{i}'
            folder.mkdir()
            (folder / 'stress-events.jsonl').write_text('\n'.join(json.dumps({
                'update_started_ticks': j * 10**7, 'updated_ticks': (j + 1) * 10**7}) for j in range(80)))
        actors = [f'{role}{i}' for role in ('w', 'r') for i in range(4)]
        self.samples = [{'cycle': i, 'before': dict.fromkeys(actors, 3 + 3 * i),
                         'after': dict.fromkeys(actors, 6 + 3 * i), 'native_before': [5, 6, 7, 8],
                         'before_errors': dict.fromkeys(actors, i), 'after_errors': dict.fromkeys(actors, i + 1)}
                        for i in range(2)]
        for actor in actors:
            (self.root / 'rust' / (actor + '.jsonl')).write_text('\n'.join(json.dumps({'event': event}) for event in
                ['ready', 'transport_read_error', 'transport_connected', 'transport_read_error', 'transport_connected', 'done']))
            with (self.root / 'rust' / (actor + '.jsonl')).open('a') as stream:
                stream.write('\n' + json.dumps({'event': 'commit' if actor.startswith('w') else 'read', 'finished_us': 1000000}))
        for actor, published in [('w0', False), ('w1', True)]:
            with (self.root / 'rust' / (actor + '.jsonl')).open('a') as stream:
                stream.write('\n' + json.dumps({'event': 'transport_commit_error', 'state': 'Unknown', 'token': actor}) + '\n')
                stream.write(json.dumps({'event': 'transport_reconciled', 'published': published, 'flush_confirmed': published, 'token': actor}) + '\n')
        (self.root / 'smb-trace.jsonl').write_text('{"cut":{}}\n{"control":{"phase":"reconnected-0"}}\n{"control":{"phase":"disconnect-1"}}\n{"cut":{}}\n{"control":{"phase":"reconnected-1"}}\n')

    def check(self):
        (self.root / 'run.json').write_text(json.dumps(self.config))
        (self.root / 'disconnect-progress.json').write_text(json.dumps(self.samples))
        with patch('native_disconnect.verify', return_value={'guarded': True}) as overlap:
            result = verify_disconnect(self.root)
            self.assertEqual([call.kwargs['phase'] for call in overlap.call_args_list], ['reconnected-0', 'reconnected-1'])
            self.assertEqual([len(call.args[0]) for call in overlap.call_args_list], [2, 5])
            return result

    def test_both_interruptions_require_post_reconnect_overlap(self):
        self.assertEqual(self.check()['interruptions'], 2)

    def test_stalled_and_backward_clients_are_rejected(self):
        for actor in ['w3', 'r2']:
            path = self.root / 'rust' / (actor + '.jsonl')
            original = path.read_text()
            for stamp in [120000001, -1]:
                path.write_text(original.replace('"finished_us": 1000000', f'"finished_us": {stamp}'))
                with self.assertRaisesRegex(AssertionError, 'progress stalled or went backwards'): self.check()
            path.write_text(original)
        path = self.root / 'n2/stress-events.jsonl'
        rows = [json.loads(line) for line in path.read_text().splitlines()]
        rows[30]['updated_ticks'] += 121 * 10**7
        path.write_text('\n'.join(map(json.dumps, rows)))
        with self.assertRaisesRegex(AssertionError, 'n2: client progress'): self.check()

    def test_reader_progress_must_cover_the_stop_barrier(self):
        stamp = 121000001000
        os.utime(self.root / 'rust/stop', ns=(stamp, stamp))
        with self.assertRaisesRegex(AssertionError, 'r0: client progress'): self.check()

    def test_one_idle_or_unaffected_client_is_rejected(self):
        original = copy.deepcopy(self.samples)
        for field, value in [('after', 3), ('after_errors', 0)]:
            self.samples = copy.deepcopy(original)
            self.samples[0][field]['r3'] = value
            with self.assertRaises(AssertionError): self.check()

    def test_missing_actor_is_rejected(self):
        for sample in self.samples:
            for field in ('before', 'after', 'before_errors', 'after_errors'): del sample[field]['r3']
        with self.assertRaises(AssertionError): self.check()

    def test_completed_native_writer_is_rejected(self):
        self.samples[0]['native_before'][2] = 80
        with self.assertRaises(AssertionError): self.check()

    def test_wrong_cut_count_is_rejected(self):
        (self.root / 'smb-trace.jsonl').write_text('{"cut":{}}\n')
        with self.assertRaises(AssertionError): self.check()

    def test_missing_reconnection_is_rejected(self):
        (self.root / 'rust/r2.jsonl').write_text('{"event":"transport_read_error"}\n')
        with self.assertRaises(AssertionError): self.check()

    def test_visibility_without_flush_confirmation_is_rejected(self):
        path = self.root / 'rust/w1.jsonl'
        path.write_text(path.read_text().replace('"flush_confirmed": true', '"flush_confirmed": false'))
        with self.assertRaisesRegex(AssertionError, 'durable acknowledgement'): self.check()

    def test_one_uncertain_outcome_is_insufficient(self):
        path = self.root / 'rust/w0.jsonl'
        path.write_text(path.read_text().replace('"state": "Unknown"', '"state": "NotCommitted"'))
        with self.assertRaisesRegex(AssertionError, 'both uncertain outcomes'): self.check()

    def test_failed_cut_observation_restores_the_connection(self):
        (self.root / 'run.json').write_text(json.dumps({**self.config, 'server': 'owned-test'}))
        (self.root / 'rust/w0.jsonl').write_text('{"event":"commit"}\n' * 3)
        controls = []
        (self.root / 'harness').mkdir()
        (self.root / 'harness/verify_smb_overlap.py').write_text('test oracle')

        def ssh(server, command, timeout):
            self.assertEqual(server, 'owned-test')
            if command.startswith('printf'):
                controls.append(json.loads(command[command.index('{'):command.index('}') + 1]))
                output = ''
            elif (failed_phase_ack and controls[-1]['phase'] == 'disconnect-0') or command.startswith('grep -c'):
                raise RuntimeError('Fault observation failed')
            else: output = json.dumps({'control': controls[-1]})
            return SimpleNamespace(stdout=output, stderr='', returncode=0, check_returncode=lambda: None)

        def capture(remote, destination, name):
            destination.write_text('{"operation":0}\n')
            return {'error': None}

        for failed_phase_ack in [False, True]:
            controls.clear()
            with self.subTest(failed_phase_ack=failed_phase_ack), \
                 patch('native_disconnect.linux_vm.run_ssh', side_effect=ssh), \
                 patch('native_disconnect.linux_vm.ssh_argv', return_value=['ssh', 'owned-test']), \
                 patch('native_disconnect.subprocess.run'), \
                 patch('native_disconnect.windows.do_get', side_effect=capture):
                with self.assertRaisesRegex(RuntimeError, 'Fault observation failed'):
                    interrupt(self.root, [{'name': 'native'}], [1], {'w0': SimpleNamespace(poll=lambda: None)})
            self.assertEqual([control['phase'] for control in controls], ['disconnect-0', 'reconnected-0'])


if __name__ == '__main__': unittest.main()
