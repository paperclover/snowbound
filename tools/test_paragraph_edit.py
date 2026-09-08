import json
from pathlib import Path
import runpy
import shutil
import subprocess
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from document_model import EXPORTER, ordered_pages, walk
from native_format import compare_formats, native_characters
from native_xml import ns
from offline_document_history import identity

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/paragraph-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class ParagraphEditTest(unittest.TestCase):
    def test_reconciled_native_edits_keep_remote_content_and_exact_graphs(self):
        fixture = FIXTURE / 'reconciliation'
        result = fixture / 'reconciled'
        self.assert_cold_graph(result)
        manifest = json.loads((result / 'manifest.json').read_text())
        self.assertEqual(len(manifest), 16)
        self.assertEqual({case['case'] for case in manifest if case['outcome'] == 'retained_conflict'},
                         {'Join remote list', 'Join empty adoption'})
        receipts = [case[key] for case in manifest for key in ('receipt', 'dependent_receipt') if key in case]
        self.assertEqual(len(set(receipts)), 28)
        models, captures = {}, {}
        with TemporaryDirectory() as temporary:
            for phase, notebook, read in [('remote', fixture / 'keyboard/notebook', fixture / 'keyboard/read'),
                                           ('reconciled', result / 'candidate', result / 'native/read')]:
                folder = Path(temporary) / phase
                subprocess.run([EXPORTER, notebook / 'synthetic.one', folder], check=True)
                model = json.loads((folder / 'document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                 for _, _, r, page in ordered_pages(model)}
                captures[phase] = {page.get('name'): native_characters(page, page.findall('one:Outline', ns))
                                   for path in read.glob('page-*.xml') for page in [ET.parse(path).getroot()]}
        for case in manifest:
            name, intent = case['case'], case['intent']
            old, page = models['remote'][name]
            new, new_page = models['reconciled'][name]
            self.assertEqual(new_page, page)
            outline, = [oid for oid in old['nodes'][page]['children'] if old['nodes'][oid]['kind']['type'] == 'Outline']
            graph = {oid: [list(node['children']), list(node['content']), node['child_level']]
                     for oid, node in walk(old, outline)}
            characters = [list(paragraph) for paragraph in captures['remote'][name]]
            if case['outcome'] != 'retained_conflict':
                if name.startswith('Split '):
                    text = intent['text']
                    paragraph, = [oid for oid, (_, content, _) in graph.items() if content == [text]]
                    parent, = [oid for oid, (children, _, _) in graph.items() if paragraph in children]
                    offset = 3 if name.endswith(('prefix', 'boundary')) else 2
                    characters[:1] = [characters[0][:offset], characters[0][offset:]]
                    characters[1] = [('C' if char == 'c' else char, style) for char, style in characters[1]]
                    right, right_text = identity(intent, 1), identity(intent, 2)
                    self.assertNotIn(right, graph)
                    self.assertNotIn(right_text, graph)
                    graph[parent][0].insert(graph[parent][0].index(paragraph) + 1, right)
                    graph[right] = [graph[paragraph][0], [right_text], graph[paragraph][2]]
                    graph[right_text] = [[], [], None]
                    graph[paragraph][0] = []
                    self.assertEqual(new['nodes'][right_text]['tags'], [])
                else:
                    left, right = intent['left'], intent['right']
                    a, = [oid for oid, (_, content, _) in graph.items() if content == [left]]
                    b, = [oid for oid, (_, content, _) in graph.items() if content == [right]]
                    parent, = [oid for oid, (children, _, _) in graph.items() if a in children]
                    graph[parent][0].remove(b)
                    graph[a][0] += graph[b][0]
                    del graph[b], graph[right]
                    characters[:2] = [characters[0] + characters[1]]
                    characters[0] = [('C' if char == 'c' else char, style) for char, style in characters[0]]
                    self.assertEqual(new['nodes'][left]['tags'], old['nodes'][left]['tags'])
            with self.subTest(case=name):
                self.assertEqual({oid: [node['children'], node['content'], node['child_level']]
                                  for oid, node in walk(new, outline)}, graph)
                for expected, actual in zip(characters, captures['reconciled'][name], strict=True):
                    for (a, before), (b, after) in zip(expected, actual, strict=True):
                        self.assertEqual(a, b)
                        for key in before.keys() | after.keys():
                            default = 'automatic' if key in ('color', 'highlight') else False
                            self.assertEqual(before.get(key, default), after.get(key, default))

    def test_native_reconciliation_controls_preserve_the_intended_edit_scope(self):
        fixture = FIXTURE / 'reconciliation'
        cases = json.loads((fixture / 'cases.json').read_text(encoding='utf-8-sig'))
        self.assertEqual(len(cases), 16)
        models, captures = {}, {}
        with TemporaryDirectory() as temporary:
            for phase in ('before', 'remote', 'keyboard'):
                folder = Path(temporary) / phase
                shutil.copytree(fixture / phase / 'read', folder / 'read')
                compare(fixture / phase / 'notebook', folder / 'read')
                subprocess.run([EXPORTER, fixture / phase / 'notebook/synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                 for _, _, r, page in ordered_pages(model)}
                captures[phase] = {page.get('name'): native_characters(page, page.findall('one:Outline', ns))
                                   for path in (folder / 'read').glob('page-*.xml') for page in [ET.parse(path).getroot()]}
                self.assertEqual(len(models[phase]), 17)
        for case in cases:
            name, change = case['name'], case['change']
            join = case['operation'] == 'join'
            old, page = models['before'][name]
            outline, = [oid for oid in old['nodes'][page]['children'] if old['nodes'][oid]['kind']['type'] == 'Outline']
            paragraphs = old['nodes'][outline]['children']
            target = paragraphs[1 if join and change in ('child', 'list', 'tag', 'format', 'right-boundary') else 0]
            expected = ['' if change == 'adoption' else 'ab🦀cd'] + (['Right'] if join else []) + ['Preserved sibling']
            self.assertEqual([old['nodes'][old['nodes'][oid]['content'][0]]['kind']['text'] for oid in paragraphs], expected)
            if change == 'prefix':
                expected[0] = 'X' + expected[0]
                if join: expected[1] += 'Y'
            elif change == 'boundary':
                expected[0] = expected[0] + 'X' if join else 'abX🦀cd'
            elif change == 'right-boundary': expected[1] = 'XRight'
            elif change == 'sibling': expected[-1] = 'Native sibling'
            elif change == 'adoption': expected[0] = 'X'
            for phase in ('remote', 'keyboard'):
                with self.subTest(case=name, phase=phase):
                    current, current_page = models[phase][name]
                    if change == 'format':
                        for char, style in captures['before'][name][int(join)]:
                            self.assertTrue(style['bold'])
                            self.assertFalse(style.get('italic', False))
                        for char, style in captures[phase][name][int(join)]:
                            self.assertTrue(style['italic'])
                            self.assertEqual(style.get('bold', False), phase == 'keyboard')
                    self.assertEqual(current_page, page)
                    active = [oid for oid, node in walk(current, outline) if node['kind']['type'] == 'Paragraph']
                    expected_ids = list(paragraphs)
                    if phase == 'keyboard' and change == 'sibling':
                        self.assertNotEqual(active[-1], paragraphs[-1])
                        expected_ids[-1] = active[-1]
                    if change == 'child':
                        child, = current['nodes'][target]['children']
                        expected_ids.insert(expected_ids.index(target) + 1, child)
                    self.assertEqual(active, expected_ids)
                    actual_texts = [current['nodes'][current['nodes'][oid]['content'][0]]['kind']['text'] for oid in active]
                    expected_texts = list(expected)
                    if change == 'child': expected_texts.insert(2 if join else 1, 'Native child')
                    self.assertEqual(actual_texts, expected_texts)
                    top = list(paragraphs)
                    if phase == 'keyboard' and change == 'sibling': top[-1] = active[-1]
                    if phase == 'keyboard' and change == 'list':
                        if join:
                            top.remove(target)
                            self.assertEqual(current['nodes'][paragraphs[0]]['children'], [target])
                        else:
                            group = current['nodes'][outline]['children'][0]
                            self.assertEqual(current['nodes'][group]['kind']['type'], 'OutlineGroup')
                            self.assertEqual(current['nodes'][group]['children'], [target])
                            self.assertEqual(current['nodes'][group]['content'], [])
                            top[0] = group
                    self.assertEqual(current['nodes'][outline]['children'], top)
                    for oid in active:
                        node = current['nodes'][oid]
                        text, = node['content']
                        if oid in paragraphs:
                            if phase == 'keyboard': self.assertEqual(node['content'], old['nodes'][oid]['content'])
                            else: self.assertNotEqual(node['content'], old['nodes'][oid]['content'])
                        child_count = int(oid == target and change == 'child')
                        if phase == 'keyboard' and join and change == 'list' and oid == paragraphs[0]: child_count = 1
                        self.assertEqual(len(node['children']), child_count)
                        self.assertEqual(len(node['kind']['lists']), int(oid == target and change == 'list'))
                        self.assertEqual(len(current['nodes'][text]['tags']), int(oid == target and change == 'tag'))

    def assert_cold_graph(self, fixture):
        with TemporaryDirectory() as temporary:
            models = []
            for source in ('candidate', 'native/notebook'):
                folder = Path(temporary) / source.replace('/', '-')
                shutil.copytree(fixture / 'native/read', folder / 'read')
                compare(fixture / source, folder / 'read')
                subprocess.run([EXPORTER, fixture / source / 'synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models.append({r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                               for _, _, r, page in ordered_pages(model)})
            self.assertEqual(models[0].keys(), models[1].keys())
            for title, (old, page) in models[0].items():
                saved, saved_page = models[1][title]
                self.assertEqual(saved_page, page)
                for outline in old['nodes'][page]['children']:
                    if old['nodes'][outline]['kind']['type'] != 'Outline': continue
                    for oid, node in walk(old, outline):
                        self.assertEqual(saved['nodes'][oid]['children'], node['children'])
                        self.assertEqual(saved['nodes'][oid]['content'], node['content'])

    def test_offline_boundaries_retain_remote_edits_and_match_native_controls(self):
        groups = [('splits', 'cold-split/read', 12), ('joins', 'joined/read', 12),
                  ('inheritance', 'join-edges/joined/read', 5), ('tags', 'join-tags/joined/read', 3)]
        for name, control, count in groups:
            fixture = FIXTURE / 'offline' / name
            manifest = json.loads((fixture / 'manifest.json').read_text())
            self.assertEqual(len(manifest), count)
            with self.subTest(group=name):
                self.assert_cold_graph(fixture)
                captures = []
                for folder in (FIXTURE / control, fixture / 'native/read'):
                    captures.append({page.get('name'): native_characters(page, page.findall('one:Outline', ns))
                                     for path in folder.glob('page-*.xml') for page in [ET.parse(path).getroot()]})
                for case in manifest:
                    self.assertTrue(case['revision'])
                    original = captures[0][case['case']]
                    current = captures[1][case['case']]
                    markers = 0
                    for old, new in zip(original, current, strict=True):
                        if new and new[0][0] == '☂':
                            markers += 1
                            new = new[1:]
                        for (a, left), (b, right) in zip(old, new, strict=True):
                            self.assertEqual(a, b)
                            for key in left.keys() | right.keys():
                                default = 'automatic' if key in ('color', 'highlight') else False
                                self.assertEqual(left.get(key, default), right.get(key, default))
                    self.assertEqual(markers, int(case['remote_prefix']))

    def test_rust_joins_match_native_controls_and_retain_cold_graph_identities(self):
        for name, control in [('split', FIXTURE / 'joined/read'),
                              ('inheritance', FIXTURE / 'join-edges/joined/read'),
                              ('tags', FIXTURE / 'join-tags/joined/read')]:
            fixture = FIXTURE / 'rust-join' / name
            manifest = json.loads((fixture / 'manifest.json').read_text())
            with self.subTest(fixture=name):
                self.assert_cold_graph(fixture)
                captures = []
                for folder in (control, fixture / 'native/read'):
                    captures.append({page.get('name'): native_characters(page, page.findall('one:Outline', ns))
                                     for path in folder.glob('page-*.xml') for page in [ET.parse(path).getroot()]})
                self.assertEqual(len(manifest), {'split': 12, 'inheritance': 5, 'tags': 3}[name])
                for case in manifest:
                    for a, b in zip(captures[0][case['case']], captures[1][case['case']], strict=True):
                        for (x, old), (y, new) in zip(a, b, strict=True):
                            self.assertEqual(x, y)
                            for key in old.keys() | new.keys():
                                default = 'automatic' if key in ('color', 'highlight') else False
                                self.assertEqual(old.get(key, default), new.get(key, default))

    def test_native_joins_preserve_inherited_styles_and_follow_tag_and_child_rules(self):
        for fixture_name, expected_cases in [('join-edges', 5), ('join-tags', 3)]:
            fixture = FIXTURE / fixture_name
            models, captures = {}, {}
            with TemporaryDirectory() as temporary:
                for phase in ('before', 'joined'):
                    folder = Path(temporary) / phase
                    shutil.copytree(fixture / phase / 'read', folder / 'read')
                    compare(fixture / phase / 'notebook', folder / 'read')
                    subprocess.run([EXPORTER, fixture / phase / 'notebook/synthetic.one', folder / 'model'], check=True)
                    model = json.loads((folder / 'model/document.json').read_text())
                    models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                     for _, _, r, page in ordered_pages(model)}
                    captures[phase] = {}
                    for path in (folder / 'read').glob('page-*.xml'):
                        page = ET.parse(path).getroot()
                        captures[phase][page.get('name')] = native_characters(page, page.findall('one:Outline', ns))
                    self.assertEqual(len(models[phase]), expected_cases + 1)
            cases = json.loads((fixture / 'cases.json').read_text(encoding='utf-8-sig'))
            self.assertEqual(len(cases), expected_cases)
            for case in cases:
                name = case['name']
                with self.subTest(case=name):
                    old, page = models['before'][name]
                    new, new_page = models['joined'][name]
                    self.assertEqual(page, new_page)
                    outline, = [oid for oid in old['nodes'][page]['children'] if old['nodes'][oid]['kind']['type'] == 'Outline']
                    left, right, sibling = old['nodes'][outline]['children']
                    self.assertEqual(new['nodes'][outline]['children'], [left, sibling])
                    target = old['nodes'][left]['children'][-1] if name == 'Join both children' else left
                    a, = old['nodes'][target]['content']
                    b, = old['nodes'][right]['content']
                    empty = old['nodes'][a]['kind']['text'] == ''
                    survivor = b if empty else a
                    self.assertEqual(new['nodes'][target]['content'], [survivor])
                    self.assertEqual(new['nodes'][survivor]['kind']['text'], old['nodes'][a]['kind']['text'] + old['nodes'][b]['kind']['text'])
                    self.assertEqual(new['nodes'][survivor]['tags'], old['nodes'][a]['tags'])
                    children = old['nodes'][left]['children'] + old['nodes'][right]['children']
                    self.assertEqual(new['nodes'][left]['children'], children)
                    self.assertEqual(new['nodes'][sibling], old['nodes'][sibling])
                    if name == 'Join both children':
                        parent_text, = old['nodes'][left]['content']
                        original = old['nodes'][parent_text]
                        current = new['nodes'][parent_text]
                        self.assertGreater(current['modified'], original['modified'])
                        expected = {**original, 'modified': current['modified'],
                                    'extra': [original['extra'][0] + [{'id': 0x880034dd, 'value': 'NoData'}]]}
                        self.assertEqual(current, expected)
                    before = [v for paragraph in captures['before'][name] for v in paragraph]
                    after = [v for paragraph in captures['joined'][name] for v in paragraph]
                    for (a, old_style), (b, new_style) in zip(before, after, strict=True):
                        self.assertEqual(a, b)
                        for key in old_style.keys() | new_style.keys():
                            default = 'automatic' if key in ('color', 'highlight') else False
                            self.assertEqual(old_style.get(key, default), new_style.get(key, default))


    def test_rust_splits_match_native_controls_and_preserve_empty_typing_styles(self):
        fixture = FIXTURE / 'rust-split'
        manifest = json.loads((fixture / 'manifest.json').read_text())
        with TemporaryDirectory() as temporary:
            for source, capture in [('candidate', 'native'), ('native/notebook', 'native'),
                                    ('typed/notebook', 'typed')]:
                native = Path(temporary) / source.replace('/', '-') / 'read'
                shutil.copytree(fixture / capture / 'read', native)
                compare(fixture / source, native)
            subprocess.run([EXPORTER, fixture / 'candidate/synthetic.one', Path(temporary) / 'model'], check=True)
            model = json.loads((Path(temporary) / 'model/document.json').read_text())
            subprocess.run([EXPORTER, fixture / 'native/notebook/synthetic.one', Path(temporary) / 'saved'], check=True)
            saved = json.loads((Path(temporary) / 'saved/document.json').read_text())
        views = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                 for _, _, r, page in ordered_pages(model)}
        saved_views = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                       for _, _, r, page in ordered_pages(saved)}
        text_nodes = {}
        for name, (revision, page) in views.items():
            current, current_page = saved_views[name]
            self.assertEqual(page, current_page)
            text_nodes[name] = []
            for outline in revision['nodes'][page]['children']:
                if revision['nodes'][outline]['kind']['type'] != 'Outline': continue
                for oid, node in walk(revision, outline):
                    self.assertEqual(current['nodes'][oid]['children'], node['children'])
                    self.assertEqual(current['nodes'][oid]['content'], node['content'])
                    if node['kind']['type'] == 'RichText': text_nodes[name].append(node)
        captures = {}
        for phase, folder in [('native', fixture / 'native/read'), ('typed', fixture / 'typed/read'),
                              ('control', FIXTURE / 'cold-split/read')]:
            captures[phase] = {}
            for path in folder.glob('page-*.xml'):
                page = ET.parse(path).getroot()
                captures[phase][page.get('name')] = native_characters(page, page.findall('one:Outline', ns))
            self.assertEqual(len(captures[phase]), 14)
        cases = [c for c in manifest['cases'] if 'intent' in c]
        self.assertEqual(len(cases), 12)
        for case in cases:
            name = case['case']
            with self.subTest(case=name):
                for a, b in zip(captures['native'][name], captures['control'][name], strict=True):
                    for (c, left), (d, right) in zip(a, b, strict=True):
                        self.assertEqual(c, d)
                        for key in left.keys() | right.keys():
                            default = 'automatic' if key in ('color', 'highlight') else False
                            self.assertEqual(left.get(key, default), right.get(key, default))
        selections = json.loads((fixture / 'typed/ui/selections.json').read_text())
        self.assertEqual(len(selections), 4)
        for selection in selections:
            node = text_nodes[selection['case']][selection['index']]
            self.assertEqual(node['kind']['text'], '')
            self.assertEqual(len(node['kind']['runs']), 1)
            node['kind']['text'] = selection['text']
            node['kind']['runs'][0]['end'] = len(selection['text'].encode('utf-16-le')) // 2
        for name, (revision, page) in views.items():
            _, differences = compare_formats(revision, text_nodes[name], captures['typed'][name])
            self.assertEqual(differences, [], name)

    def test_native_split_join_graphs_styles_and_identity_boundaries(self):
        manifest = json.loads((FIXTURE / 'manifest.json').read_text())
        models, native = {}, {}
        with TemporaryDirectory() as temporary:
            for phase in manifest['phases']:
                folder = Path(temporary) / phase
                read = folder / 'read'
                shutil.copytree(FIXTURE / phase / 'read', read)
                compare(FIXTURE / phase / 'notebook', read)
                subprocess.run([EXPORTER, FIXTURE / phase / 'notebook/synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind'].get('title'): (r, page)
                                 for _, _, r, page in ordered_pages(model)}
                self.assertEqual(len(models[phase]), 14)
                native[phase] = {}
                for file in read.glob('page-*.xml'):
                    page = ET.parse(file).getroot()
                    native[phase][page.get('name')] = native_characters(page, page.findall('one:Outline', ns))
        self.assertEqual(len(manifest['cases']), 13)
        for case in manifest['cases']:
            name = case['case']
            with self.subTest(case=name):
                old, page = models['before'][name]
                text, paragraph, suffix, suffix_text = (case[key] for key in
                    ('original_text', 'original_paragraph', 'new_paragraph', 'new_text'))
                parent, = [oid for oid, node in old['nodes'].items() if paragraph in node['children']]
                position = old['nodes'][parent]['children'].index(paragraph)
                expected_children = old['nodes'][parent]['children'].copy()
                expected_children.insert(position + 1, suffix)
                encoded = old['nodes'][text]['kind']['text'].encode('utf-16-le')
                at = case['offset_utf16'] * 2
                for phase in ('split', 'cold-split'):
                    current, _ = models[phase][name]
                    self.assertEqual(current['nodes'][parent]['children'], expected_children)
                    self.assertEqual(current['nodes'][paragraph]['content'], [text])
                    self.assertEqual(current['nodes'][paragraph]['children'], [])
                    self.assertEqual(current['nodes'][suffix]['content'], [suffix_text])
                    self.assertEqual(current['nodes'][suffix]['children'], old['nodes'][paragraph]['children'])
                    self.assertEqual(current['nodes'][text]['kind']['text'], encoded[:at].decode('utf-16-le'))
                    self.assertEqual(current['nodes'][suffix_text]['kind']['text'], encoded[at:].decode('utf-16-le'))
                    self.assertEqual(current['nodes'][text]['tags'], old['nodes'][text]['tags'])
                    self.assertEqual(current['nodes'][suffix_text]['tags'], [])
                joined, joined_page = models['joined'][name]
                adopted = name in ('Split start', 'Split empty')
                self.assertEqual(joined['nodes'][page]['children'], old['nodes'][page]['children'])
                self.assertEqual(page, joined_page)
                for outline in old['nodes'][page]['children']:
                    if old['nodes'][outline]['kind']['type'] != 'Outline': continue
                    for oid, node in walk(old, outline):
                        if adopted and oid == text: continue
                        self.assertEqual(joined['nodes'][oid]['children'], node['children'])
                        content = [suffix_text] if adopted and oid == paragraph else node['content']
                        self.assertEqual(joined['nodes'][oid]['content'], content)
                        if node['kind']['type'] == 'RichText':
                            self.assertEqual(joined['nodes'][oid]['kind']['text'], node['kind']['text'])
                            self.assertEqual(joined['nodes'][oid]['tags'], node['tags'])
                self.assertEqual(joined['nodes'][suffix_text if adopted else text]['kind']['text'], encoded.decode('utf-16-le'))
                for left, right in zip(native['before'][name], native['joined'][name], strict=True):
                    for (a, old_style), (b, new_style) in zip(left, right, strict=True):
                        self.assertEqual(a, b)
                        for key in old_style.keys() | new_style.keys():
                            default = 'automatic' if key in ('color', 'highlight') else False
                            self.assertEqual(old_style.get(key, default), new_style.get(key, default))


if __name__ == '__main__':
    unittest.main()
