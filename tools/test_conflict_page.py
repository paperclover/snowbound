"""Conflict pages (corpus/conflict-page): what OneNote 2010 stores and shows for one, and
that it reads the ones Rust writes and deletes."""
import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest
import hashlib

from native_xml import pages, texts
from document_model import EXPORTER, ordered_pages, view

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/conflict-page'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']
DEFAULT = '{00000000-0000-0000-0000-000000000000},0'
RENDER, SELECT = 0x88001D96, 0x88001DDB


def model(notebook, section='synthetic.one'):
    with TemporaryDirectory() as temporary:
        output = Path(temporary) / 'model'
        subprocess.run([EXPORTER, notebook / section, output], check=True, capture_output=True)
        return json.loads((output / 'document.json').read_text())


def conflicts(document):
    """Each listed page's conflict pages: whose version, its texts and the marked texts."""
    listed = {}
    for sid, _, page, _ in ordered_pages(document):
        manifest = page['nodes'][page['roots']['1']]
        versions = []
        for space in manifest['spaces']:
            _, revision = view(document, space)
            metadata = revision['nodes'][revision['roots']['2']]['kind']
            rich = [node for node in revision['nodes'].values() if node['kind']['type'] == 'RichText']
            flagged = lambda node, flag: any(field['id'] == flag for fields in node['extra'] for field in fields)
            versions.append({'user': metadata['author'], 'texts': sorted(node['kind']['text'] for node in rich),
                             'marked': sorted(node['kind']['text'] for node in rich if flagged(node, RENDER) and flagged(node, SELECT))})
        listed[sid] = versions
    return listed


def page_texts(document, sid):
    _, page = view(document, sid)
    return sorted(node['kind']['text'] for node in page['nodes'].values() if node['kind']['type'] == 'RichText')


TITLES = {'One', 'Two', 'Three', 'Four', 'Target'}


def series(document):
    """The section's page series in order, each as the titles of its pages."""
    _, root = view(document, document['root'])
    section = root['nodes'][root['roots']['1']]
    return {oid: [next((text for text in page_texts(document, space) if text in TITLES), '')
                  for space in root['nodes'][oid]['spaces']] for oid in section['children']}


def spaces(document):
    """Each listed page's space by its title."""
    return {next((text for text in page_texts(document, sid) if text in TITLES), ''): sid
            for sid, *_ in ordered_pages(document)}


class ConflictPageTest(unittest.TestCase):
    def test_onenote_keeps_the_remote_version_and_the_local_one_as_a_conflict_page(self):
        document = model(FIXTURE / 'native/conflict/notebook')
        (sid, versions), = conflicts(document).items()
        # Client A's disjoint edit merged; the paragraph both changed keeps client B's text.
        self.assertIn('Client A disjoint edit.', page_texts(document, sid))
        self.assertIn('Client B conflicting edit.', page_texts(document, sid))
        self.assertEqual(versions, [{'user': 'snow',
                                     'texts': ['', 'Client A conflicting edit.', 'Client A disjoint edit.', 'Left cell', 'Right cell'],
                                     'marked': ['Client A conflicting edit.']}])
        # COM lists the page alone; the conflict page shows only through its bar.
        for client in 'ab':
            self.assertTrue((FIXTURE / f'native/{client}/{client}-shown.png').is_file())

    def test_onenote_deletes_a_conflict_page_from_its_page_only(self):
        document = model(FIXTURE / 'native-delete/notebook')
        self.assertEqual(conflicts(document), {sid: [] for sid in conflicts(document)})
        self.assertEqual(len(pages(FIXTURE / 'native-delete/read')), 1)

    def test_three_clients_conflict_pages_list_the_last_stored_first(self):
        document = model(FIXTURE / 'native-three/notebook')
        (_, versions), = conflicts(document).items()
        self.assertEqual([version['user'] for version in versions],
                         ['ONE-M6-DDE10E0D', 'ONE-M6-9451B6C6', 'ONE-M6-053BD357'])

    def test_onenote_reads_a_rust_conflict_page_shows_its_bar_and_deletes_it(self):
        source = json.loads((FIXTURE / 'cold/source.json').read_text())
        written = hashlib.sha256((FIXTURE / 'candidate/synthetic.one').read_bytes()).hexdigest()
        self.assertIn(written, [item['sha256'] for item in source])
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/before-read', read)
            compare(FIXTURE / 'candidate', read)
        (page,) = pages(FIXTURE / 'cold/before-read')
        self.assertIn('Remote: café, 東京, مرحبا', texts(page))
        document = model(FIXTURE / 'candidate')
        (_, versions), = conflicts(document).items()
        self.assertEqual(versions[0]['user'], 'Clover Snow')
        self.assertEqual(versions[0]['marked'], ['Snowbound: café, 東京, مرحبا'])
        # The bar on the page opens the conflict page, as OneNote's own do, and its menu
        # deletes it, leaving the page without a bar.
        titles = {name: json.loads((FIXTURE / f'cold/cold-{name}.json').read_text())['win']['title']
                  for name in ('main', 'shown', 'delete')}
        self.assertEqual(titles, {'main': 'Remote: café, 東京, مرحبا - Microsoft OneNote',
                                  'shown': 'Snowbound: café, 東京, مرحبا - Microsoft OneNote',
                                  'delete': 'Remote: café, 東京, مرحبا - Microsoft OneNote'})
        deleted = model(FIXTURE / 'cold/notebook')
        self.assertEqual(conflicts(deleted), {sid: [] for sid in conflicts(deleted)})

    def test_onenote_reads_a_conflict_page_rust_deleted(self):
        document = model(FIXTURE / 'candidate-delete')
        self.assertEqual(conflicts(document), {sid: [] for sid in conflicts(document)})
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold-delete/read', read)
            compare(FIXTURE / 'candidate-delete', read)
        (page,) = pages(FIXTURE / 'cold-delete/read')
        self.assertIn('Client B conflicting edit.', texts(page))

    def test_page_moves_merge_as_page_series_do(self):
        """Each client gives a page it moves a series of its own; the merge keeps each page
        in the series of the client that gave it one, the server's where both did, and
        leaves the other client's series empty. A page the server deleted comes back in a
        new series where the offline client had it."""
        for capture, order, empty, server, kept, client in (
                ('native-pages', ['One', 'Two', 'Four', 'Three', 'Target'], 2, ['One', 'Two', 'Four'], [], ['Three', 'Target']),
                ('native-restore', ['Four', 'One', 'Two', 'Target', 'Three'], 0, [], ['Four'], ['One', 'Two', 'Three', 'Target'])):
            root = FIXTURE / capture
            initial, published, merged = (series(model(root / phase / 'notebook'))
                                          for phase in ('initial', 'b-published', 'merged'))
            self.assertEqual([titles for titles in merged.values() if titles][1:], [[title] for title in order])
            self.assertEqual(sum(not titles for titles in merged.values()), empty, capture)
            self.assertEqual(json.loads((root / 'merged.json').read_text())['order'][1:], order)
            for listed in 'ab':
                self.assertEqual(json.loads((root / listed / 'listed.json').read_text())[1:], order)
            owner = lambda state, title: next((oid for oid, titles in state.items() if title in titles), None)
            for title in server:
                self.assertEqual(owner(merged, title), owner(published, title), title)
                self.assertNotIn(owner(merged, title), initial, title)
            for title in kept:
                self.assertEqual(owner(merged, title), owner(initial, title), title)
            for title in client:
                self.assertNotIn(owner(merged, title), {**initial, **published}, title)

    def test_a_page_edited_offline_and_deleted_online_comes_back_as_a_new_page(self):
        for capture in ('native-pages', 'native-restore'):
            root = FIXTURE / capture
            before = spaces(model(root / 'initial/notebook'))
            document = model(root / 'merged/notebook')
            target = spaces(document)['Target']
            self.assertNotEqual(target, before['Target'], capture)
            self.assertIn('Body Target edited offline.', page_texts(document, target))
            # The deleting client's version stays in the recycle bin.
            deleted = model(root / 'merged/notebook/OneNote_RecycleBin', 'OneNote_DeletedPages.one')
            self.assertIn('Body Target.', page_texts(deleted, spaces(deleted)['Target']), capture)


if __name__ == '__main__':
    unittest.main()
