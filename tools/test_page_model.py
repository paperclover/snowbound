"""Rust page-model edits survive a cold native reopen and a native edit on top of them."""
import json
from pathlib import Path
import runpy
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER, ordered_pages, walk

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-model'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def projection(space, page_root):
    """Outlines with their positions and pre-order paragraphs (text, parent index, collapsed)."""
    page = space['nodes'][page_root]
    outlines = []
    for oid in page['children']:
        node = space['nodes'][oid]
        if node['kind']['type'] != 'Outline':
            continue
        paragraphs = []
        index = {}
        pending = [(child, None) for child in reversed(node['children'])]
        while pending:
            pid, parent = pending.pop()
            paragraph = space['nodes'][pid]
            if paragraph['kind']['type'] == 'OutlineGroup':
                pending.extend((child, parent) for child in reversed(paragraph['children']))
                continue
            assert paragraph['kind']['type'] == 'Paragraph', paragraph['kind']['type']
            text = None
            for cid in paragraph['content']:
                content = space['nodes'][cid]
                if content['kind']['type'] == 'RichText':
                    text = content['kind']['text']
            index[pid] = len(paragraphs)
            paragraphs.append({'text': text, 'parent': index.get(parent), 'collapsed': paragraph['kind'].get('collapse_state') == 1})
            pending.extend((child, pid) for child in reversed(paragraph['children']))
        layout = node['layout']
        outlines.append({'x': points(layout['x']), 'y': points(layout['y']), 'max_width': points(layout['max_width']),
                         'width_set_by_user': layout.get('width_set_by_user'), 'paragraphs': paragraphs})
    return outlines


def points(value):
    """Single-precision points survive JSON with different digits in Rust and Python."""
    return None if value is None else round(value, 3)


def expected_projection(case):
    return [{'x': points(o['x']), 'y': points(o['y']), 'max_width': points(o['max_width']), 'width_set_by_user': o['width_set_by_user'],
             'paragraphs': [{'text': p['text'], 'parent': p['parent'], 'collapsed': p['collapsed']} for p in o['paragraphs']]}
            for o in case['expected']['outlines']]


def model(notebook):
    with TemporaryDirectory() as temporary:
        subprocess.run([EXPORTER, notebook / 'synthetic.one', Path(temporary) / 'model'], check=True)
        return json.loads((Path(temporary) / 'model/document.json').read_text())


class PageModelTest(unittest.TestCase):
    def setUp(self):
        self.manifest = json.loads((FIXTURE / 'candidate/manifest.json').read_text())
        self.assertEqual(len(self.manifest['cases']), 8)

    def check_cases(self, notebook, exceptions=()):
        pages = list(ordered_pages(model(notebook)))
        self.assertEqual(len(pages), 15)
        for case in self.manifest['cases']:
            with self.subTest(case=case['title']):
                _, _, space, root = pages[case['index']]
                actual = projection(space, root)
                expected = expected_projection(case)
                for index, replacement in exceptions:
                    if index == case['index']:
                        expected = replacement(expected)
                self.assertEqual(actual, expected)

    def assert_only_navigation_cache_changed(self, candidate, cold):
        """OneNote adds per-series metadata copies on open (as it does for the untouched
        outline-edit fixture); every page space must keep its exact active revision."""
        context = '{00000000-0000-0000-0000-000000000000},0'
        self.assertEqual(set(candidate['spaces']), set(cold['spaces']))
        for sid, space in candidate['spaces'].items():
            if sid == candidate['root']:
                continue
            self.assertEqual(space['contexts'][context], cold['spaces'][sid]['contexts'][context], sid)
        before = candidate['spaces'][candidate['root']]
        after = cold['spaces'][cold['root']]
        before = before['revisions'][before['contexts'][context]]['nodes']
        after = after['revisions'][after['contexts'][context]]['nodes']
        self.assertTrue(set(before) <= set(after))
        for oid, node in before.items():
            other = after[oid]
            if node['kind']['type'] == 'Metadata':
                continue
            if node['kind']['type'] == 'PageSeries':
                strip = lambda n: {k: v for k, v in n.items() if k != 'extra'}
                self.assertEqual(strip(node), strip(other), oid)
                continue
            self.assertEqual(node, other, oid)
        for oid in set(after) - set(before):
            self.assertEqual(after[oid]['kind']['type'], 'Metadata', oid)

    def test_rust_page_model_edits_survive_cold_native_reopen(self):
        compare(FIXTURE / 'candidate/notebook', FIXTURE / 'cold/read')
        compare(FIXTURE / 'cold/notebook', FIXTURE / 'cold/read')
        self.assert_only_navigation_cache_changed(model(FIXTURE / 'candidate/notebook'), model(FIXTURE / 'cold/notebook'))
        self.check_cases(FIXTURE / 'candidate/notebook')
        self.check_cases(FIXTURE / 'cold/notebook')

    def test_native_edit_after_rust_page_model_edits(self):
        compare(FIXTURE / 'followup-cold/notebook', FIXTURE / 'followup-cold/read')
        native = (FIXTURE / 'followup/native-text.txt').read_text(encoding='utf-8-sig')
        self.assertTrue(native.startswith('Native after Rust'))
        moved = next(case for case in self.manifest['cases'] if case['title'] == 'Move leaf down')

        def replace_rust_text(outlines):
            outlines = json.loads(json.dumps(outlines))
            hits = [p for o in outlines for p in o['paragraphs'] if p['text'].startswith('Rust ')]
            assert len(hits) == 1
            hits[0]['text'] = native
            return outlines

        self.check_cases(FIXTURE / 'followup-cold/notebook', exceptions=[(moved['index'], replace_rust_text)])


if __name__ == '__main__':
    unittest.main()
