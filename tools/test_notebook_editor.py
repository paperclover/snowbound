import json
from pathlib import Path
import tempfile
from threading import Thread
import unittest
from unittest.mock import patch
from urllib.error import HTTPError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from document_model import view, walk
import notebook_editor as editor
from random_edit_campaign import export, verify


class EditorTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        source = editor.ROOT / 'corpus/native/20260905-05/snapshots/06-attachment/notebook'
        self.session = editor.Session(source, Path(self.temporary.name) / 'session')
        self.server = editor.ThreadingHTTPServer(('127.0.0.1', 0), editor.Handler)
        self.server.session = self.session
        self.thread = Thread(target=self.server.serve_forever)
        self.thread.start()
        self.addCleanup(self.stop)
        self.url = f'http://127.0.0.1:{self.server.server_port}'
        self.file = self.session.output / 'notebook/synthetic.one'
        self.before = export(self.file)
        self.row = json.loads((self.session.output / 'g/0/report/pages.json').read_text())[0]
        _, revision = view(self.before[0], self.row['space'])
        self.oid, node = next((oid, node) for oid, node in walk(revision, self.row['object'])
                              if node['kind']['type'] == 'RichText' and node['kind']['text'].startswith('Fictitious'))
        self.selection = {'generation': 0, 'page': self.row['report'], 'object': self.oid, 'run': 0, 'action': 'text'}
        self.text = self.before[1][self.row['space']][self.row['revision']][self.oid][0]['text']

    def stop(self):
        self.server.shutdown()
        self.thread.join()
        self.server.server_close()

    def request(self, path, data=None):
        request = Request(self.url + path, data=json.dumps(data).encode() if data is not None else None,
                          headers={'Content-Type': 'application/json', 'X-OneNote-Diagnostic': '1'})
        try:
            response = urlopen(request)
        except HTTPError as error:
            response = error
        with response:
            return response.status, json.loads(response.read())

    def test_unicode_stale_snapshot_and_unrelated_content(self):
        status, checked = self.request('/api/run?' + urlencode(self.selection))
        self.assertEqual((status, checked['ok'], checked['text']), (200, True, self.text))
        replacement = self.text + ' café 🦀 e\u0301 <diagnostic>'
        status, saved = self.request('/api/save', {**self.selection, 'replacement': replacement})
        self.assertEqual((status, saved['state'], saved['ok']), (200, 'Committed', True))
        intent, outcome = [json.loads(s) for s in (self.session.output / 'operations.jsonl').read_text().splitlines()]
        edit = intent['edit']
        verify(self.before, export(self.file), {'page': self.row['object'], 'space': edit['space'], 'object': self.oid,
               'range': [edit['action']['start'], edit['action']['end']], 'replacement': replacement, 'run_start': edit['action']['start'],
               'run_before': self.text, 'started_ms': intent['started_ms'], 'finished_ms': outcome['finished_ms']})
        after = self.file.read_bytes()
        fresh = self.session.output / 'fresh-report'
        editor.generate(self.session.output / 'g/1/snapshot', fresh, editable=True)
        cached = self.session.output / 'g/1/report'
        self.assertEqual({p.relative_to(fresh): p.read_bytes() for p in fresh.rglob('*') if p.is_file()},
                         {p.relative_to(cached): p.read_bytes() for p in cached.rglob('*') if p.is_file()})
        status, stale = self.request('/api/save', {**self.selection, 'replacement': 'another draft'})
        self.assertEqual((status, stale['state'], stale['kind']), (409, 'NotCommitted', 'ResourceBusy'))
        self.assertEqual(self.file.read_bytes(), after)
        self.assertEqual((self.session.output / 'g/0/snapshot/synthetic.one').read_bytes(),
                         (editor.ROOT / 'corpus/native/20260905-05/snapshots/06-attachment/notebook/synthetic.one').read_bytes())
        self.assertEqual((self.session.output / 'g/1/snapshot/synthetic.one').stat().st_ino,
                         (self.session.output / 'g/2/snapshot/synthetic.one').stat().st_ino)
        with urlopen(self.url + saved['location']) as response:
            html = response.read().decode()
        self.assertIn('&lt;diagnostic&gt;', html)
        self.assertNotIn('<diagnostic>', html)

    def test_invalid_edits_and_selection_never_write(self):
        before = self.file.read_bytes()
        for changes in [{'replacement': 'first\nsecond'}, {'replacement': '\ud800'},
                        {'run': -1}, {'generation': True}, {'object': 'missing'}, {'extra': 'field'}]:
            status, result = self.request('/api/save', {**self.selection, 'replacement': 'changed', **changes})
            self.assertEqual((status, result['state']), (422, 'NotCommitted'))
            self.assertEqual(self.file.read_bytes(), before)

    def test_unknown_response_does_not_replay_a_real_commit(self):
        original = editor.bridge
        calls = []
        def uncertain(mode, *args):
            result = original(mode, *args)
            if mode == 'commit':
                calls.append(result)
                self.assertTrue(result['ok'])
                return {'ok': False, 'state': 'Unknown', 'error': 'Simulated lost outcome'}
            return result
        with patch.object(editor, 'bridge', side_effect=uncertain):
            status, result = self.request('/api/save', {**self.selection, 'replacement': self.text + ' once'})
        self.assertEqual((status, result['state'], len(calls)), (503, 'Unknown', 1))
        after = export(self.file)
        rid, _ = view(after[0], self.row['space'])
        self.assertEqual(after[1][self.row['space']][rid][self.oid][0]['text'], self.text + ' once')
        status, stale = self.request('/api/save', {**self.selection, 'replacement': self.text + ' twice'})
        self.assertEqual((status, stale['kind']), (409, 'ResourceBusy'))

    def test_refresh_failure_preserves_committed_outcome(self):
        with patch.object(self.session, 'snapshot', side_effect=OSError('Report storage unavailable')):
            status, result = self.request('/api/save', {**self.selection, 'replacement': self.text + ' saved'})
        self.assertEqual((status, result['state'], result['ok']), (503, 'Committed', False))
        self.assertIn('Report storage', result['report_error'])
        after = export(self.file)
        rid, _ = view(after[0], self.row['space'])
        self.assertEqual(after[1][self.row['space']][rid][self.oid][0]['text'], self.text + ' saved')
        with urlopen(self.url + '/latest?' + urlencode(self.selection)) as response:
            self.assertIn(' saved', response.read().decode())

    def test_document_actions_preserve_placement_unicode_and_unselected_formatting(self):
        status, page = self.request('/api/page?' + urlencode(self.selection))
        self.assertEqual(status, 200)
        outline = next(target for target in page['targets'] if target['label'].startswith('Outline'))
        base = {'generation': 0, 'page': self.row['report'], 'object': outline['object'], 'action': 'paragraph',
                'before': outline['children'][0]['object'], 'text': 'A🦀 café\rsecond line', 'author': 'Diagnostic test'}
        status, saved = self.request('/api/save', base)
        self.assertEqual((status, saved['state']), (200, 'Committed'))
        after = export(self.file)
        _, revision = view(after[0], self.row['space'])
        children = revision['nodes'][outline['object']]['children']
        self.assertEqual(children[1], base['before'])
        oid, node = next((key, node) for key, node in walk(revision, children[0]) if node['kind']['type'] == 'RichText')
        self.assertEqual(node['kind']['text'], base['text'])
        self.assertEqual(self.before[2], after[2])
        attrs = [{'Bold': True}, {'Italic': True}, {'Underline': True}, {'Strike': True},
                 {'Superscript': True}, {'Subscript': False}, {'Font': 'Arial'}, {'FontSize': 20.5},
                 {'Color': [18, 52, 86]}, {'Highlight': [255, 255, 0]}]
        selected = {'generation': 1, 'page': saved['location'].split('/')[-1], 'object': oid,
                    'run': 0, 'action': 'format', 'start': 1, 'end': 3, 'attributes': attrs}
        status, formatted = self.request('/api/save', selected)
        self.assertEqual((status, formatted['state']), (200, 'Committed'))
        after_format = export(self.file)
        rid, _ = view(after_format[0], self.row['space'])
        runs = after_format[1][self.row['space']][rid][oid]
        self.assertEqual([run['text'] for run in runs], ['A', '🦀', ' café\rsecond line'])
        original_rid, _ = view(after[0], self.row['space'])
        original_format = after[1][self.row['space']][original_rid][oid][0]['format']
        self.assertEqual(runs[0]['format'], original_format)
        self.assertEqual(runs[2]['format'], original_format)
        for key, value in {'bold': True, 'italic': True, 'underline': True, 'strike': True,
                           'superscript': True, 'subscript': False, 'font': 'Arial', 'font_size': 20.5,
                           'color': 0x563412, 'highlight': 0xffff}.items():
            self.assertEqual(runs[1]['format'][key], value, key)
        status, cleared = self.request('/api/save', {**selected, 'generation': 2, 'run': 1, 'start': 0, 'end': 2,
                                                      'attributes': [{'Subscript': True}, {'Color': None}, {'Highlight': None}]})
        self.assertEqual(status, 200)
        cleared_model = export(self.file)
        rid, _ = view(cleared_model[0], self.row['space'])
        fmt = cleared_model[1][self.row['space']][rid][oid][1]['format']
        self.assertEqual((fmt['superscript'], fmt['subscript'], fmt['color'], fmt['highlight']), (False, True, 0xff000000, 0xff000000))
        status, added = self.request('/api/save', {'generation': 3, 'page': selected['page'], 'action': 'outline',
                                                   'object': self.row['object'], 'x': 216.5, 'y': 360,
                                                   'text': 'New outline 🦀', 'author': 'Diagnostic test'})
        self.assertEqual(status, 200)
        _, revision = view(export(self.file)[0], self.row['space'])
        new_outline = revision['nodes'][revision['nodes'][self.row['object']]['children'][-1]]
        self.assertEqual(new_outline['kind']['type'], 'Outline')
        self.assertEqual((new_outline['layout']['x'], new_outline['layout']['y']), (216.5, 360))
        committed = self.file.read_bytes()
        for request in (base, selected):
            status, stale = self.request('/api/save', request)
            self.assertEqual((status, stale['state'], stale['kind']), (409, 'NotCommitted', 'ResourceBusy'))
            self.assertEqual(self.file.read_bytes(), committed)

    def test_document_rejections_do_not_publish(self):
        before = self.file.read_bytes()
        common = {'generation': 0, 'page': self.row['report'], 'object': self.oid}
        requests = [
            {**common, 'action': 'format', 'run': 0, 'start': 0, 'end': 1, 'attributes': []},
            {**common, 'action': 'format', 'run': 0, 'start': True, 'end': 1, 'attributes': [{'Bold': True}]},
            {**common, 'action': 'format', 'run': 0, 'start': 0, 'end': 99999, 'attributes': [{'Bold': True}]},
            {**common, 'action': 'format', 'run': 0, 'start': 0, 'end': 1, 'attributes': [{'FontSize': 144}]},
            {**common, 'action': 'format', 'run': 0, 'start': 0, 'end': 1, 'attributes': [{'Bold': True}, {'Bold': False}]},
            {**common, 'action': 'outline', 'x': 1, 'y': 1, 'text': 'No page parent', 'author': 'test'},
            {**common, 'action': 'paragraph', 'before': None, 'text': 'No paragraph parent', 'author': 'test'},
        ]
        for request in requests:
            status, result = self.request('/api/save', request)
            self.assertEqual((status, result['state']), (422, 'NotCommitted'))
            self.assertEqual(self.file.read_bytes(), before)

    def test_uncertain_insertion_has_one_publication_and_a_stale_retry(self):
        request = {'generation': 0, 'page': self.row['report'], 'object': self.row['object'],
                   'action': 'outline', 'x': 144, 'y': 288, 'text': 'Only once 🦀', 'author': 'test'}
        original = editor.bridge
        def uncertain(mode, *args):
            result = original(mode, *args)
            return {'ok': False, 'state': 'Unknown'} if mode == 'commit' and result['ok'] else result
        with patch.object(editor, 'bridge', side_effect=uncertain):
            status, result = self.request('/api/save', request)
        self.assertEqual((status, result['state']), (503, 'Unknown'))
        status, result = self.request('/api/save', request)
        self.assertEqual((status, result['kind']), (409, 'ResourceBusy'))
        _, revision = view(export(self.file)[0], self.row['space'])
        self.assertEqual(sum(n['kind'].get('text') == 'Only once 🦀' for _, n in walk(revision, self.row['object'])), 1)

    def test_generated_fields_are_read_only(self):
        source = editor.ROOT / 'corpus/m6/native-structure-01/notebook'
        session = editor.Session(source, Path(self.temporary.name) / 'fields')
        self.server.session = session
        pages = json.loads((session.output / 'g/0/report/pages.json').read_text())
        document = export(source / 'synthetic.one')[0]
        for row in pages:
            _, revision = view(document, row['space'])
            fields = [oid for oid, node in walk(revision, row['object'])
                      if node['kind']['type'] == 'RichText' and node['kind']['boilerplate']]
            if fields: break
        self.assertTrue(fields)
        status, result = self.request('/api/run?' + urlencode({'generation': 0, 'page': row['report'], 'object': fields[0], 'run': 0}))
        self.assertEqual((status, result['ok']), (422, False))

    def test_templates_keep_reader_only_controls(self):
        session = editor.Session(editor.ROOT / 'corpus/m6/native-template-controls-01/notebook',
                                 Path(self.temporary.name) / 'template')
        self.server.session = session
        pages = json.loads((session.output / 'g/0/report/pages.json').read_text())
        protected = [row for row in pages if row['category'] == 'Default page template']
        self.assertTrue(protected)
        for row in protected:
            status, result = self.request('/api/run?' + urlencode({'generation': 0, 'page': row['report'], 'object': row['object'], 'run': 0}))
            self.assertEqual(status, 422)
            self.assertIn('active page', result['error'])
            html = (session.output / 'g/0/report' / row['report']).read_text()
            self.assertNotIn('src="/editor.js"', html)


if __name__ == '__main__':
    unittest.main()
