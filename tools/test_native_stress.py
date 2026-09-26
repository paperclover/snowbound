import copy
from contextlib import contextmanager
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch
import native_stress
from native_stress import edit_history, native_history, verify_capture
from native_collaboration import reachable_page_text
from document_model import DEFAULT_CONTEXT, EXPORTER


class NativeHistoryTests(unittest.TestCase):
    def test_conflict_oracle_excludes_unreferenced_history_and_spaces(self):
        root = Path(__file__).resolve().parent.parent
        with tempfile.TemporaryDirectory() as folder:
            output = Path(folder) / 'model'
            subprocess.run([EXPORTER,
                root / 'corpus/collaboration/round-01/offline/notebook/synthetic.one', output], check=True)
            model = json.loads((output / 'document.json').read_text())
        retained = reachable_page_text(model)
        self.assertEqual(list(retained.values()), [['Rust edited during the native outage.'],
                                                   ['Native edited while disconnected.']])
        main, conflict = retained
        current = model['spaces'][main]
        revision = current['revisions'][current['contexts'][DEFAULT_CONTEXT]]
        current['revisions']['unreferenced'] = copy.deepcopy(revision)
        for node in current['revisions']['unreferenced']['nodes'].values():
            if node['kind']['type'] == 'RichText': node['kind']['text'] = 'History is not recovery'
        self.assertEqual(reachable_page_text(model), retained)
        revision['nodes'][revision['roots']['1']]['spaces'] = []
        self.assertIn(conflict, model['spaces'])
        self.assertEqual(reachable_page_text(model), {main: retained[main]})
        page, = revision['nodes'][revision['roots']['1']]['content']
        revision['nodes'][page]['children'] *= 2
        self.assertEqual(reachable_page_text(model), {main: retained[main] * 2})

    def setUp(self):
        self.logs = {}
        text = 'Concurrent edits:'
        for i in range(2):
            token = f' [w{i}:0]'
            at = len(text.encode('utf-16-le')) // 2
            self.logs[f'w{i}'] = [{'event': 'ready'},
                {'event': 'intent', 'attempt': 1, 'operation': 0, 'token': token,
                 'replacement': token, 'before': text, 'range': [at, at], 'source_transaction': 1},
                {'event': 'commit', 'attempt': 1, 'operation': 0, 'token': token,
                 'source_transaction': 1, 'started_us': i * 20, 'finished_us': i * 20 + 10},
                {'event': 'done'}]
            text += token
        self.logs['r0'] = [{'event': 'ready'}, {'event': 'read', 'text': text,
            'started_us': 40, 'finished_us': 50}, {'event': 'done'}]

    def test_incomplete_capture_requires_explicit_retention_mode(self):
        self.logs['w0'].pop()
        with self.assertRaisesRegex(AssertionError, 'Incomplete client log'):
            edit_history(self.logs, 2)
        self.assertEqual(len(edit_history(self.logs, 2, partial=True)[0]), 2)
        self.logs['w1'][2]['operation'] = 1
        with self.assertRaisesRegex(AssertionError, 'acknowledgements'):
            edit_history(self.logs, 2, partial=True)
        self.logs['w1'][2]['operation'] = 0
        self.logs['w1'].pop(2)
        with self.assertRaisesRegex(AssertionError, 'reader observed'):
            edit_history(self.logs, 2, partial=True)

    def test_counter_renumbering_preserves_one_content_history(self):
        commits, text = edit_history(self.logs, 1)
        self.assertEqual(len(commits), 2)
        self.assertEqual(text, self.logs['r0'][1]['text'])

    def test_branches_unexplained_edits_and_incorrect_observations_fail(self):
        for field, value in [('before', 'Concurrent edits:'), ('before', 'unknown'),
                             ('range', [0, 0]), ('replacement', 'wrong')]:
            logs = copy.deepcopy(self.logs)
            logs['w1'][1][field] = value
            with self.assertRaises(AssertionError): edit_history(logs, 1)
        for field, value in [('text', 'Concurrent edits:'), ('text', 'unknown'), ('finished_us', 1)]:
            logs = copy.deepcopy(self.logs)
            logs['r0'][1][field] = value
            with self.assertRaises(AssertionError): edit_history(logs, 1)

    def test_content_branch_and_disconnected_history_are_detected(self):
        for before, message in [('Concurrent edits:', 'branched'), ('Concurrent edits: unexplained', 'complete content history')]:
            logs = copy.deepcopy(self.logs)
            logs['w1'][1]['before'] = before
            offset = len(before.encode('utf-16-le')) // 2
            logs['w1'][1]['range'] = [offset, offset]
            with self.assertRaisesRegex(AssertionError, message): edit_history(logs, 1)

    def test_replacements_chain_by_content_after_native_counter_renumbering(self):
        expected = 'Concurrent edits:'
        for i in range(2):
            events = self.logs[f'w{i}']
            intent = events[1]
            intent['before'] = expected
            intent['range'] = [len('Concurrent edits:'), len(expected.encode('utf-16-le')) // 2]
            intent['replacement'] = ' café 🦀' + intent['token']
            expected = 'Concurrent edits:' + intent['replacement']
        self.logs['r0'][1]['text'] = expected
        commits, text = edit_history(self.logs, 1, edit=True)
        self.assertEqual((len(commits), text), (2, expected))
        self.logs['w1'][1]['range'][0] = len('Concurrent edits: café ') + 1
        with self.assertRaises(UnicodeDecodeError): edit_history(self.logs, 1, edit=True)

    def test_missing_acknowledgement_is_not_silently_dropped(self):
        self.logs['w1'].pop(2)
        with self.assertRaises(AssertionError): edit_history(self.logs, 1)

    def test_native_replacement_history_rejects_lost_edits_and_split_surrogates(self):
        events = [{'operation': 0, 'token': ' [n0:0]', 'before': 'Native 0:',
                   'range': [9, 9], 'replacement': ' café 🦀 [n0:0]'},
                  {'operation': 1, 'token': ' [n0:1]', 'before': 'Native 0: café 🦀 [n0:0]',
                   'range': [10, 17], 'replacement': ' café 🦀 [n0:1]'}]
        self.assertEqual(native_history(events, 0, 2, True), 'Native 0:  café 🦀 [n0:1] [n0:0]')
        for field, value in [('before', 'Native 0:'), ('range', [15, 16]),
                             ('operation', 0), ('replacement', 'unrecorded')]:
            changed = copy.deepcopy(events)
            changed[1][field] = value
            with self.assertRaises((AssertionError, UnicodeDecodeError)):
                native_history(changed, 0, 2, True)
        with self.assertRaises(AssertionError): native_history(events[:-1], 0, 2, True)

    def test_cold_capture_rejects_extra_missing_and_misformatted_content(self):
        with tempfile.TemporaryDirectory() as folder:
            root = Path(folder)
            (root / 'rust').mkdir()
            (root / 'n0').mkdir()
            (root / 'run.json').write_text(json.dumps({'stress_operations': 1, 'edit': True,
                'rust_writers': 2, 'rust_readers': 1, 'stress_clients': 1}))
            text = 'Concurrent edits:'
            for i in range(2):
                events = self.logs[f'w{i}']
                events[1].update(before=text, range=[17, len(text.encode('utf-16-le')) // 2],
                                 replacement=' café 🦀' + events[1]['token'])
                text = 'Concurrent edits:' + events[1]['replacement']
            self.logs['r0'][1]['text'] = text
            for actor, events in self.logs.items():
                (root / 'rust' / f'{actor}.jsonl').write_text('\n'.join(map(json.dumps, events)))
            event = {'operation': 0, 'token': ' [n0:0]', 'before': 'Native 0:',
                     'range': [9, 9], 'replacement': ' café 🦀 [n0:0]', 'bold': True, 'italic': False}
            (root / 'n0/stress-events.jsonl').write_text(json.dumps(event), encoding='utf-8-sig')
            native = '<OE style="font-weight:bold"><T>Native 0: café 🦀 [n0:0]</T></OE>'
            body = f'<OE><T>{text}</T></OE>{native}'
            page = root / 'page-000.xml'
            page.write_text(f'<Page xmlns="http://schemas.microsoft.com/office/onenote/2010/onenote"><Outline>{body}</Outline></Page>')
            result = verify_capture(root, root)
            self.assertEqual((result['rust_intents'], result['native_intents'], result['exact_paragraphs']), (2, 1, 2))
            self.assertGreater(result['native_intended_format_checks'], 0)
            for changed in (body + native, body.replace(native, ''), body.replace('bold', 'normal')):
                page.write_text(f'<Page xmlns="http://schemas.microsoft.com/office/onenote/2010/onenote"><Outline>{changed}</Outline></Page>')
                with self.assertRaises(AssertionError): verify_capture(root, root)


class FailureArtifacts(unittest.TestCase):
    def test_failed_clients_preserve_native_logs_without_masking_the_failure(self):
        @contextmanager
        def failed_clients(*args, **kwargs):
            yield {}
            raise RuntimeError('Rust client exited')

        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory)
            (output / 'run.json').write_text(json.dumps({'maintenance': False}))
            shared = output / 'shared'
            shared.mkdir()
            clients = [{'name': f'n{i}', 'folder': output / f'n{i}'} for i in range(2)]
            for client in clients: client['folder'].mkdir()

            def capture(remote, local, name):
                self.assertTrue(remote.endswith('\\outbox\\7\\events.jsonl'))
                if name == 'n0': raise OSError('Native client unavailable')
                local.write_text('{"operation":0}\n')
                return {'error': None}

            with patch.object(native_stress, 'running_clients', failed_clients), \
                 patch.object(native_stress.windows, 'do_health', return_value={'utc_us': 0}), \
                 patch.object(native_stress.windows, 'do_cmd', return_value={'stdout': 'ready editing'}), \
                 patch.object(native_stress.windows, 'do_get', side_effect=capture):
                with self.assertRaisesRegex(RuntimeError, 'Rust client exited'):
                    native_stress.exercise(output, shared, clients, lambda *a, **kw: 7,
                        lambda *a: None, lambda *a: None, lambda *a: None, 40, 1, 4, 4)
            self.assertEqual(json.loads((output / 'n0/stress-capture.json').read_text())['error'], 'Native client unavailable')
            self.assertEqual((output / 'n1/stress-events.jsonl').read_text(), '{"operation":0}\n')
