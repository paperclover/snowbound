import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest

from document_model import EXPORTER, ordered_pages, view, walk

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/page-lifecycle'
PHASES = ('01-blank', '02-authored', '03-renamed', '04-nested', '05-reordered', '06-deleted-parent')
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class PageLifecycleTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        temporary = TemporaryDirectory()
        cls.addClassCleanup(temporary.cleanup)
        cls.models = {}
        for phase in PHASES:
            for version in ('', 'cold'):
                output = Path(temporary.name) / phase / (version or 'source')
                output.parent.mkdir(parents=True, exist_ok=True)
                subprocess.run([EXPORTER, FIXTURE / phase / version / 'notebook/Lifecycle.one', output], check=True)
                cls.models[phase, version] = json.loads((output / 'document.json').read_text())

    def test_each_native_phase_survives_an_independent_cold_reopen(self):
        for phase, count in zip(PHASES, (3, 9, 9, 9, 9, 8), strict=True):
            with self.subTest(phase=phase), TemporaryDirectory() as temporary:
                source = Path(temporary) / 'notebook'
                source.mkdir()
                (source / 'Lifecycle.one').symlink_to(FIXTURE / phase / 'notebook/Lifecycle.one')
                native = Path(temporary) / 'read'
                cold = Path(temporary) / 'cold/read'
                shutil.copytree(FIXTURE / phase / 'read', native)
                shutil.copytree(FIXTURE / phase / 'cold/read', cold)
                compare(source, native)
                for version in ('', 'cold'):
                    compare(FIXTURE / phase / version / 'notebook', cold)
                before = list(ordered_pages(self.models[phase, '']))
                after = list(ordered_pages(self.models[phase, 'cold']))
                self.assertEqual(len(before), count)
                self.assertEqual(len(after), count)
                for (sid, _, old, page), (new_sid, _, new, new_page) in zip(before, after, strict=True):
                    self.assertEqual((sid, page), (new_sid, new_page))
                    self.assertEqual(dict(walk(old, page)), dict(walk(new, page)))
                    self.assertEqual(old['nodes'][old['roots']['2']]['kind'], new['nodes'][new['roots']['2']]['kind'])
                groups = []
                for version in ('', 'cold'):
                    document = self.models[phase, version]
                    _, section = view(document, document['root'])
                    groups.append([section['nodes'][oid]['spaces']
                                   for oid in section['nodes'][section['roots']['1']]['children']])
                self.assertEqual(groups[0], groups[1])

    def test_atomic_rust_nesting_matches_native_and_survives_cold_reopen(self):
        fixture = FIXTURE / 'atomic-nesting'
        self.assertEqual((fixture / 'candidate/Lifecycle.one').read_bytes()[1024:],
                         (fixture / 'cold/notebook/Lifecycle.one').read_bytes()[1024:])
        with TemporaryDirectory() as temporary:
            native_read = Path(temporary) / 'native/read'
            cold_read = Path(temporary) / 'cold/read'
            shutil.copytree(FIXTURE / '04-nested/read', native_read)
            shutil.copytree(fixture / 'cold/read', cold_read)
            models = []
            for ordinal, source in enumerate((fixture / 'candidate', fixture / 'cold/notebook')):
                output = Path(temporary) / str(ordinal)
                subprocess.run([EXPORTER, source / 'Lifecycle.one', output], check=True)
                models.append(json.loads((output / 'document.json').read_text()))
                compare(source, native_read)
                compare(source, cold_read)
            pages = [list(ordered_pages(model)) for model in models]
            expected = list(ordered_pages(self.models['04-nested', '']))
            original = list(ordered_pages(self.models['03-renamed', '']))
            self.assertEqual(len(pages[0]), 9)
            for before, native, candidate, cold in zip(original, expected, *pages, strict=True):
                sid, _, revision, page = candidate
                self.assertEqual((sid, page), (before[0], before[3]))
                self.assertEqual((sid, page), (cold[0], cold[3]))
                self.assertEqual(dict(walk(revision, page)), dict(walk(before[2], before[3])))
                self.assertEqual(revision['nodes'], cold[2]['nodes'])
                self.assertEqual(revision['roots'], cold[2]['roots'])
                metadata = revision['nodes'][revision['roots']['2']]['kind']
                self.assertEqual(metadata, native[2]['nodes'][native[2]['roots']['2']]['kind'])
            groups = []
            for model in (self.models['04-nested', ''], *models):
                _, revision = view(model, model['root'])
                groups.append([revision['nodes'][oid]['spaces']
                               for oid in revision['nodes'][revision['roots']['1']]['children']])
            self.assertEqual(groups[0], groups[1])
            self.assertEqual(groups[1], groups[2])
            _, candidate = view(models[0], models[0]['root'])
            _, cold = view(models[1], models[1]['root'])
            self.assertEqual(candidate['nodes'], cold['nodes'])
            self.assertEqual(candidate['roots'], cold['roots'])

    def test_native_page_styles_titles_order_and_parent_deletion(self):
        phases = {phase: list(ordered_pages(self.models[phase, ''])) for phase in PHASES}
        initial = [sid for sid, _, _, _ in phases['02-authored']]
        for ordinal, (_, _, revision, page) in enumerate(phases['01-blank']):
            self.assertEqual(revision['nodes'][page]['children'], [])
            self.assertEqual(bool(revision['nodes'][page]['structure']), ordinal < 2)
        titles = [''] * 3 + ['Same title', 'child', 'grandchild', 'Same title', 'trailing', 'Body automatic 🦀 é preserved']
        self.assertEqual([r['nodes'][r['roots']['2']]['kind']['title'] for _, _, r, _ in phases['02-authored']], titles)
        titles[6:8] = ['Renamed 🦋 é', 'Body trailing 🦀 é preserved']
        self.assertEqual([r['nodes'][r['roots']['2']]['kind']['title'] for _, _, r, _ in phases['03-renamed']], titles)
        for phase in ('03-renamed', '04-nested'):
            self.assertEqual([sid for sid, _, _, _ in phases[phase]], initial)
        for phase, order, levels, groups in [
            ('04-nested', list(range(9)), [1, 1, 1, 1, 2, 3, 1, 1, 1], [1, 1, 1, 3, 1, 1, 1]),
            ('05-reordered', [0, 1, 2, 6, 3, 4, 5, 7, 8], [1, 1, 1, 1, 1, 2, 3, 1, 1], [1, 1, 1, 1, 3, 1, 1]),
            ('06-deleted-parent', [0, 1, 2, 6, 4, 5, 7, 8], [1, 1, 1, 1, 2, 3, 1, 1], [1, 1, 1, 3, 1, 1]),
        ]:
            self.assertEqual([sid for sid, _, _, _ in phases[phase]], [initial[i] for i in order])
            self.assertEqual([r['nodes'][r['roots']['2']]['kind']['level'] for _, _, r, _ in phases[phase]], levels)
            document = self.models[phase, '']
            _, section = view(document, document['root'])
            series = section['nodes'][section['roots']['1']]['children']
            self.assertEqual([len(section['nodes'][oid]['spaces']) for oid in series], groups)
        original = {sid: (r, page) for sid, _, r, page in phases['02-authored']}
        for phase in PHASES[2:]:
            for sid, _, revision, page in phases[phase]:
                before, old_page = original[sid]
                self.assertEqual(page, old_page)
                self.assertEqual(before['nodes'][page]['children'], revision['nodes'][page]['children'])
                for outline in before['nodes'][page]['children']:
                    self.assertEqual(dict(walk(before, outline)), dict(walk(revision, outline)))
        before = {sid: (r, page) for sid, _, r, page in phases['03-renamed']}
        for phase in ('04-nested', '05-reordered', '06-deleted-parent'):
            for sid, _, revision, page in phases[phase]:
                old, old_page = before[sid]
                self.assertEqual(dict(walk(old, old_page)), dict(walk(revision, page)))
        with TemporaryDirectory() as temporary:
            output = Path(temporary) / 'deleted'
            subprocess.run([EXPORTER, FIXTURE / '06-deleted-parent/notebook/OneNote_RecycleBin/OneNote_DeletedPages.one', output], check=True)
            document = json.loads((output / 'document.json').read_text())
            (_, _, deleted, page), = ordered_pages(document)
            _, _, parent, parent_page = phases['05-reordered'][4]
            self.assertEqual(deleted['nodes'][deleted['roots']['2']]['kind']['title'], 'Same title')
            self.assertEqual([node['kind']['text'] for _, node in walk(deleted, page) if node['kind']['type'] == 'RichText'],
                             [node['kind']['text'] for _, node in walk(parent, parent_page) if node['kind']['type'] == 'RichText'])


if __name__ == '__main__':
    unittest.main()
