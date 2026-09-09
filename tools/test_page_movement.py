import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER, ordered_pages, view, walk
from native_page_movement import PHASES
from verify_page_creation import compare

FIXTURE = Path(__file__).resolve().parent.parent / 'corpus/page-lifecycle'


class PageMovementTest(unittest.TestCase):
    def test_native_selection_indentation_and_order_survive_cold_reopen(self):
        with TemporaryDirectory() as temporary:
            temporary = Path(temporary)
            subprocess.run([EXPORTER, FIXTURE / '04-nested/notebook/Lifecycle.one', temporary / 'baseline'], check=True)
            baseline = json.loads((temporary / 'baseline/document.json').read_text())
            original = list(ordered_pages(baseline))
            expected = [(phase, order, levels) for phase, _, _, order, levels in PHASES]
            expected.append(('single-page', [0, 1, 2, 4, 5, 6, 7, 8, 3], [1, 1, 1, 2, 3, 1, 1, 1, 1]))
            for phase, order, levels in expected:
                with self.subTest(phase=phase):
                    fixture = FIXTURE / 'movement' / phase
                    if phase == '02-promoted-parent':
                        with self.assertRaises(AssertionError):
                            compare(fixture / 'notebook', fixture / 'cold')
                    refreshed = compare(fixture / 'notebook', fixture / 'cold',
                                        refresh_metadata_levels=phase == '02-promoted-parent')
                    self.assertEqual(refreshed, int(phase == '02-promoted-parent'))
                    output = temporary / phase
                    subprocess.run([EXPORTER, fixture / 'notebook/Lifecycle.one', output], check=True)
                    document = json.loads((output / 'document.json').read_text())
                    pages = list(ordered_pages(document))
                    self.assertEqual([(sid, page) for sid, _, _, page in pages],
                                     [(original[i][0], original[i][3]) for i in order])
                    self.assertEqual([r['nodes'][r['roots']['2']]['kind']['level'] for _, _, r, _ in pages], levels)
                    groups = []
                    for (sid, _, revision, page), index, level in zip(pages, order, levels, strict=True):
                        if level == 1:
                            groups.append([])
                        groups[-1].append(sid)
                        old = dict(walk(original[index][2], original[index][3]))
                        current = dict(walk(revision, page))
                        self.assertEqual(old.keys(), current.keys())
                        for oid in old:
                            for field in ('children', 'content', 'structure', 'format'):
                                self.assertEqual(old[oid][field], current[oid][field], (phase, oid, field))
                            if oid != page:
                                self.assertEqual(old[oid]['kind'], current[oid]['kind'], (phase, oid))
                    _, section = view(document, document['root'])
                    self.assertEqual([section['nodes'][oid]['spaces']
                                      for oid in section['nodes'][section['roots']['1']]['children']], groups)


if __name__ == '__main__':
    unittest.main()
