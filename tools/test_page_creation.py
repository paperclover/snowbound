import json
from pathlib import Path
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER, ordered_pages, walk
from verify_page_creation import compare

FIXTURE = Path(__file__).resolve().parent.parent / 'corpus/page-lifecycle/creation'


class PageCreationTest(unittest.TestCase):
    def test_created_pages_and_both_writers_survive_native_cold_reopens(self):
        for source, capture in [('candidate', 'cold'), ('native/notebook', 'native/cold'), ('followup', 'followup/cold')]:
            with self.subTest(source=source):
                compare(FIXTURE / source, FIXTURE / capture)

    def test_duplicate_titles_keep_distinct_pages_through_native_and_rust_edits(self):
        with TemporaryDirectory() as temporary:
            identities = None
            for ordinal, path in enumerate(('candidate', 'native/cold/notebook', 'followup/cold/notebook')):
                output = Path(temporary) / str(ordinal)
                subprocess.run([EXPORTER, FIXTURE / path / 'Lifecycle.one', output], check=True)
                document = json.loads((output / 'document.json').read_text())
                pages = list(ordered_pages(document))
                self.assertEqual(len(pages), 13)
                current = [(sid, page) for sid, _, _, page in pages]
                self.assertEqual(len(set(current)), 13)
                if identities is None:
                    identities = current
                self.assertEqual(identities, current)
                for index, (_, _, view, page) in enumerate(pages[-2:]):
                    title = ('Reviewed ' if ordinal == 2 else '') + 'New 🦋 é'
                    self.assertEqual(view['nodes'][view['roots']['2']]['kind']['title'], title)
                    expected = [title]
                    if ordinal:
                        expected.append(('Rust + ' if ordinal == 2 else '') + f'Native body {index} after Rust page creation.')
                    self.assertEqual([node['kind']['text'] for _, node in walk(view, page)
                                      if node['kind']['type'] == 'RichText'], expected)
                for at, titled in [(0, True), (10, False)]:
                    _, _, view, page = pages[at]
                    self.assertEqual(bool(view['nodes'][page]['structure']), titled)
                    self.assertEqual(view['nodes'][page]['children'], [])


if __name__ == '__main__':
    unittest.main()
