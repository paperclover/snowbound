"""Page versions (corpus/page-versions): what OneNote 2010 stores for them and does to them,
and that it reads, lists and deletes the ones Rust writes."""
import hashlib
import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER, ordered_pages, version_pages, walk

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-versions'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
HISTORY = '{7111497F-1B6B-4209-9491-C98B04CF4C5A},1'
HAS_VERSION_PAGES = 0x88003462


def model(notebook):
    with TemporaryDirectory() as temporary:
        output = Path(temporary) / 'model'
        subprocess.run([EXPORTER, notebook / 'History.one', output], check=True, capture_output=True)
        return json.loads((output / 'document.json').read_text())


# The title's date and time fields, the same on every state of the page.
DATE = {'Sunday, September 27, 2026', '5:42 PM'}


def texts(space, root):
    return [node['kind']['text'] for _, node in walk(space, root)
            if node['kind']['type'] == 'RichText' and node['kind']['text'] and node['kind']['text'] not in DATE]


def versions(document):
    """Each listed page's text and versions, newest first: each version's author and text,
    and whether its metadata says it has versions."""
    listed = []
    for sid, _, page, oid in ordered_pages(document):
        own = []
        history = document['spaces'][sid]['revisions'].get(document['spaces'][sid]['contexts'].get(HISTORY))
        for _, _, revision, version, proxy in version_pages(document, sid, page):
            author = history['nodes'][proxy['latest_author']]['kind']['name']
            own.append((author, texts(revision, version)))
        metadata = page['nodes'][page['roots']['2']]
        flagged = any(field['id'] == HAS_VERSION_PAGES for fields in metadata['extra'] for field in fields)
        listed.append((texts(page, oid), own, flagged))
    return listed


FIRST = 'First state. Typed by virtual.'
SECOND = 'Second author line.'


class PageVersionsTest(unittest.TestCase):
    def test_onenote_keeps_a_version_when_another_author_edits(self):
        native = FIXTURE / 'native'
        self.assertEqual(versions(model(native / 'step-01/notebook')),
                         [(['Versioned', FIRST], [], False)])
        page = ['Versioned', FIRST, SECOND]
        self.assertEqual(versions(model(native / 'step-02/notebook')),
                         [(page, [('virtual', ['Versioned', FIRST])], True)])
        # Copy Page To copies the version as a page of its own, without versions.
        self.assertEqual(versions(model(native / 'step-03/notebook'))[1],
                         (['Versioned', FIRST], [], False))
        # Restore Version: the page as it stood becomes the newest version.
        self.assertEqual(versions(model(native / 'step-04/notebook'))[0],
                         (['Versioned', FIRST], [('Other Person', page), ('virtual', ['Versioned', FIRST])], True))
        # Delete Version, then Delete All Versions in Section, which clears the flag.
        self.assertEqual(versions(model(native / 'step-05/notebook'))[0][1], [('virtual', ['Versioned', FIRST])])
        self.assertEqual(versions(model(native / 'step-06/notebook'))[0], (['Versioned', FIRST], [], False))
        # Optimizing keeps versions.
        self.assertEqual(len(versions(model(native / 'step-10/notebook'))[0][1]), 1)

    def test_the_scripted_capture_repeats_it(self):
        scripted = FIXTURE / 'native-scripted'
        counts = [len(versions(model(scripted / f'step-{n:02}/notebook'))[0][1]) for n in range(1, 10)]
        self.assertEqual(counts, [0, 1, 1, 2, 1, 0, 1, 0, 1])

    def test_onenote_reads_a_rust_restore_and_deletes_its_version(self):
        for name in ('restore', 'delete-all'):
            source = json.loads((FIXTURE / f'cold-{name}/source.json').read_text())
            written = hashlib.sha256((FIXTURE / f'candidate-{name}/History.one').read_bytes()).hexdigest()
            self.assertIn(written, [item['sha256'] for item in source])
            with TemporaryDirectory() as temporary:
                read = Path(temporary) / 'read'
                shutil.copytree(FIXTURE / f'cold-{name}/before-read', read)
                compare(FIXTURE / f'candidate-{name}', read)
        restored = versions(model(FIXTURE / 'candidate-restore'))[0]
        self.assertEqual(restored[0], ['Versioned', FIRST])
        self.assertEqual([author for author, _ in restored[1]], ['Other Person', 'virtual'])
        self.assertEqual(versions(model(FIXTURE / 'candidate-delete-all'))[0], (['Versioned', FIRST], [], False))
        # OneNote listed the Rust versions under the page, opened the newest under its bar and
        # deleted it through the bar's menu, leaving the older one.
        record = lambda name: json.loads((FIXTURE / f'cold-restore/shots/{name}.json').read_text())
        self.assertEqual(record('cold-version')['win']['title'], 'Versioned - Microsoft OneNote')
        after = versions(model(FIXTURE / 'cold-restore/notebook'))[0]
        self.assertEqual([author for author, _ in after[1]], ['virtual'])

if __name__ == '__main__':
    unittest.main()
