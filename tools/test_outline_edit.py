import copy
import json
from pathlib import Path
import runpy
import shutil
import struct
import subprocess
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from document_model import EXPORTER, ordered_pages, walk
from native_format import native_characters
from native_xml import ns, Text

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/outline-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class OutlineEditTest(unittest.TestCase):
    def test_rust_subtree_moves_and_deletions_survive_cold_native_reopen(self):
        for name, pages in [
            ('rust-tree/ordinary', 15), ('rust-tree/groups-cells', 12),
            ('rust-tree/cross-container', 12), ('rust-tree/unequal-groups', 1),
            ('offline-tree', 15), ('offline-tree/cell-delete', 12), ('offline-tree/cell-move', 12),
        ]:
            fixture = FIXTURE / name
            with self.subTest(case=name), TemporaryDirectory() as temporary:
                models = {}
                for phase, notebook in [('candidate', fixture / 'candidate'), ('cold', fixture / 'cold/notebook')]:
                    folder = Path(temporary) / phase
                    shutil.copytree(fixture / 'cold/read', folder / 'read')
                    compare(notebook, folder / 'read')
                    section, = notebook.glob('*.one')
                    subprocess.run([EXPORTER, section, folder / 'model'], check=True)
                    model = json.loads((folder / 'model/document.json').read_text())
                    models[phase] = {sid: (r, page) for sid, _, r, page in ordered_pages(model)}
                    self.assertEqual(len(models[phase]), pages)
                self.assertEqual(models['candidate'].keys(), models['cold'].keys())
                for sid, (old, page) in models['candidate'].items():
                    saved, saved_page = models['cold'][sid]
                    self.assertEqual(page, saved_page)
                    self.assertEqual(dict(walk(old, page)), dict(walk(saved, page)))
                    self.assertEqual(old['nodes'][old['roots']['2']]['kind'], saved['nodes'][saved['roots']['2']]['kind'])
                    for _, node in walk(old, page):
                        for oid in node['kind'].get('lists', []):
                            self.assertEqual(old['nodes'][oid], saved['nodes'][oid])
                if name == 'rust-tree/unequal-groups':
                    capture, = (fixture / 'cold/read').glob('page-*.xml')
                    root = ET.parse(capture).getroot()
                    self.assertEqual([(group.get('indent'), ''.join(Text(group.find('one:OE/one:T', ns).text or '').parts))
                                      for group in root.findall('one:Outline/one:OEChildren', ns)],
                                     [('3', 'First'), ('2', 'Second')])

    def test_native_group_list_and_cell_tree_controls(self):
        fixture = FIXTURE / 'tree'
        models = {}
        with TemporaryDirectory() as temporary:
            for phase in ('before', 'after', 'cold'):
                folder = Path(temporary) / phase
                shutil.copytree(fixture / phase / 'read', folder / 'read')
                compare(fixture / phase / 'notebook', folder / 'read')
                subprocess.run([EXPORTER, fixture / phase / 'notebook/synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                 for _, _, r, page in ordered_pages(model)}
                self.assertEqual(len(models[phase]), 12)
        cases = json.loads((fixture / 'cases.json').read_text(encoding='utf-8-sig'))
        self.assertEqual(len(cases), 11)
        for name, (saved, page) in models['after'].items():
            cold, cold_page = models['cold'][name]
            self.assertEqual(page, cold_page)
            self.assertEqual(dict(walk(saved, page)), dict(walk(cold, cold_page)))
            for _, node in walk(saved, page):
                for oid in node['kind'].get('lists', []):
                    self.assertEqual(saved['nodes'][oid], cold['nodes'][oid])
        for case in cases:
            name = case['name']
            with self.subTest(case=name):
                old, page = models['before'][name]
                new, new_page = models['after'][name]
                self.assertEqual(page, new_page)
                self.assertEqual(old['nodes'][page]['children'], new['nodes'][page]['children'])
                outline, other = [oid for oid in old['nodes'][page]['children']
                                  if old['nodes'][oid]['kind']['type'] == 'Outline']
                original = dict(walk(old, outline))
                expected = copy.deepcopy(original)
                actual = dict(walk(new, outline))
                paragraphs = {old['nodes'][n['content'][0]]['kind']['text']: oid
                              for oid, n in original.items() if n['kind']['type'] == 'Paragraph'
                              and n['content'] and old['nodes'][n['content'][0]]['kind']['type'] == 'RichText'}
                target, = [oid for text, oid in paragraphs.items() if text.startswith('Target ')]
                parents = {child: oid for oid, node in original.items() for child in node['children']}
                parent = parents[target]
                if name.startswith('Delete'):
                    for oid, _ in walk(old, target):
                        del expected[oid]
                    expected[parent]['children'].remove(target)
                    if name == 'Delete only grouped subtree':
                        del expected[parent]
                        expected[outline]['children'].remove(parent)
                    elif name == 'Delete unindented sibling after group':
                        group, = expected[outline]['children']
                        expected[outline]['children'] = expected[group]['children']
                        expected[outline]['child_level'] = 2
                        del expected[group]
                    elif name == 'Delete sole cell paragraph':
                        replacement, = actual[parent]['children']
                        self.assertNotIn(replacement, original)
                        blank = actual[replacement]
                        self.assertEqual(blank['kind'], {'type': 'Paragraph', 'lists': [],
                                                        'paragraph_style': None, 'collapse_state': None})
                        self.assertEqual((blank['children'], blank['tags'], blank['child_level']), ([], [], 1))
                        text, = blank['content']
                        self.assertNotIn(text, original)
                        self.assertEqual(actual[text]['kind'], {'type': 'RichText', 'text': '',
                                         'runs': [{'start': 0, 'end': 0, 'format': None, 'extra_set': None}],
                                         'paragraph_style': None, 'boilerplate': False})
                        expected[parent]['children'] = [replacement]
                        expected.update({replacement: blank, text: actual[text]})
                elif name in ('Move numbered subtree down', 'Move cell subtree down'):
                    children = expected[parent]['children']
                    index = children.index(target)
                    children[index:index + 2] = reversed(children[index:index + 2])
                    if name == 'Move numbered subtree down':
                        expected[paragraphs['Anchor']]['child_level'] = 2
                elif name == 'Indent first subtree':
                    group, = actual.keys() - original.keys()
                    self.assertEqual(actual[group]['kind'], {'type': 'OutlineGroup'})
                    self.assertEqual(actual[group]['children'], [target])
                    self.assertEqual((actual[group]['child_level'], actual[group]['content'],
                                      actual[group]['structure'], actual[group]['tags']), (1, [], [], []))
                    expected[group] = actual[group]
                    expected[outline]['children'][0] = group
                elif name == 'Outdent first group':
                    del expected[parent]
                    expected[outline]['children'][0] = target
                elif name == 'Indent bullet subtree':
                    expected[outline]['children'].remove(target)
                    expected[paragraphs['Anchor']]['children'] = [target]
                    before_list, = expected[target]['kind']['lists']
                    after_list, = actual[target]['kind']['lists']
                    self.assertNotEqual(before_list, after_list)
                    self.assertEqual(old['nodes'][before_list]['kind'], {'type': 'List', 'font': 'Courier New',
                                     'format': '○', 'restart': None, 'bullet': 4})
                    self.assertEqual(new['nodes'][after_list]['kind'], {'type': 'List', 'font': 'Calibri',
                                     'format': '•', 'restart': None, 'bullet': 1})
                    expected[target]['kind']['lists'] = [after_list]
                elif name == 'Outdent child across indentation gap':
                    self.assertEqual(expected[parent]['child_level'], 2)
                    expected[parent]['child_level'] = 1
                    expected[parent]['children'].remove(paragraphs['Other child'])
                    expected[target]['children'].append(paragraphs['Other child'])
                else:
                    self.fail(name)
                if case['shape'] not in ('cell', 'cell-only'):
                    reservation = actual[outline]['extra'][0][-1]
                    self.assertEqual(reservation['id'], 0x14001cdb)
                    width, = struct.unpack('<f', bytes.fromhex(reservation['value']['Bytes']))
                    self.assertAlmostEqual(width * 36, 423.75, places=3)
                    expected[outline]['extra'][0].append(reservation)
                self.assertEqual(actual.keys(), expected.keys())
                for oid, node in expected.items():
                    self.preserved_node(node, actual[oid])
                    for list_id in node['kind'].get('lists', []):
                        if list_id in old['nodes']:
                            self.preserved_node(old['nodes'][list_id], new['nodes'][list_id])
                for oid, node in walk(old, other):
                    self.preserved_node(node, new['nodes'][oid])

    def cold_layout(self, fixture):
        models = {}
        with TemporaryDirectory() as temporary:
            for phase, notebook in [('candidate', fixture / 'candidate'), ('cold', fixture / 'cold/notebook')]:
                folder = Path(temporary) / phase
                shutil.copytree(fixture / 'cold/read', folder / 'read')
                compare(notebook, folder / 'read')
                subprocess.run([EXPORTER, notebook / 'synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                 for _, _, r, page in ordered_pages(model)}
        captures = {page.get('name'): page for path in (fixture / 'cold/read').glob('page-*.xml')
                    for page in [ET.parse(path).getroot()]}
        self.assertEqual(len(models['candidate']), 15)
        self.assertEqual(models['candidate'].keys(), models['cold'].keys())
        for name, (old, page) in models['candidate'].items():
            new, saved_page = models['cold'][name]
            self.assertEqual(page, saved_page)
            for oid, node in walk(old, page):
                actual = new['nodes'][oid]
                for key in ('children', 'content', 'structure', 'child_level', 'layout'):
                    self.assertEqual(actual[key], node[key], (name, oid, key))
                self.assertEqual(actual['kind'].get('collapse_state'), node['kind'].get('collapse_state'))
                if node['kind']['type'] == 'RichText':
                    self.assertEqual(actual['kind'], node['kind'])
        return models, captures

    def test_rust_layout_and_saved_expansion_survive_cold_native_reopen(self):
        models, captures = self.cold_layout(FIXTURE / 'layout')
        cases = json.loads((FIXTURE / 'cases.json').read_text(encoding='utf-8-sig'))
        selected = [case for case in cases if any(key in case for key in ('position', 'size', 'collapse'))]
        self.assertEqual(len(selected), 5)
        for case in selected:
            name = case['name']
            old, page = models['candidate'][name]
            saved, _ = models['cold'][name]
            oid = next(oid for oid in old['nodes'][page]['children'] if old['nodes'][oid]['kind']['type'] == 'Outline')
            node = saved['nodes'][oid]
            z = old['nodes'][page]['children'].index(oid)
            native = next(outline for outline in captures[name].findall('one:Outline', ns)
                          if int(outline.find('one:Position', ns).get('z')) == z)
            for key, value in case.get('position', {}).items():
                self.assertEqual(node['layout'][key], value)
            if 'size' in case:
                size = native.find('one:Size', ns)
                self.assertEqual(node['layout']['max_width'], case['size']['width'])
                self.assertEqual(size.get('isSetByUser') in ('true', '1'), case['size']['isSetByUser'])
                if case['size']['isSetByUser']:
                    self.assertAlmostEqual(float(size.get('width')), case['size']['width'], places=3)
            if 'collapse' in case:
                target, = [p for p in native.findall('.//one:OE', ns) if p.find('one:T', ns) is not None
                           and ''.join(Text(p.find('one:T', ns).text or '').parts).startswith('Target ')]
                self.assertEqual(target.get('collapsed') in ('true', '1'), case['collapse'])

    def test_rust_width_replaces_native_reserved_wrap_width(self):
        models, captures = self.cold_layout(FIXTURE / 'reserved-width')
        saved, page = models['cold']['Move outline']
        oid = next(oid for oid in saved['nodes'][page]['children'] if saved['nodes'][oid]['kind']['type'] == 'Outline')
        node = saved['nodes'][oid]
        self.assertEqual(node['layout']['max_width'], 144)
        self.assertTrue(node['layout']['width_set_by_user'])
        self.assertFalse(any(field['id'] == 0x14001cdb for field in node['extra'][0]))
        z = saved['nodes'][page]['children'].index(oid)
        native = next(outline for outline in captures['Move outline'].findall('one:Outline', ns)
                      if int(outline.find('one:Position', ns).get('z')) == z)
        size = native.find('one:Size', ns)
        self.assertEqual(size.get('isSetByUser'), 'true')
        self.assertAlmostEqual(float(size.get('width')), 144, places=3)

    def test_offline_native_reconciliation_preserves_graph_and_dependent_edits(self):
        models, _ = self.cold_layout(FIXTURE / 'offline')
        records = json.loads((FIXTURE / 'offline/cases.json').read_text())
        self.assertEqual(len(records), 14)
        self.assertEqual(sum(row['retained'] for row in records), 4)
        records = {row['name']: row for row in records}
        with TemporaryDirectory() as temporary:
            folder = Path(temporary)
            subprocess.run([EXPORTER, FIXTURE / 'after/notebook/synthetic.one', folder / 'model'], check=True)
            model = json.loads((folder / 'model/document.json').read_text())
            remote = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                      for _, _, r, page in ordered_pages(model)}
        changed = texts = 0
        for name, (candidate, page) in models['candidate'].items():
            original, remote_page = remote[name]
            self.assertEqual(page, remote_page)
            record = records.get(name)
            for oid, node in walk(candidate, page):
                before = original['nodes'][oid]
                for key in ('children', 'content', 'structure', 'child_level'):
                    self.assertEqual(node[key], before[key], (name, oid, key))
                layout = dict(before['layout'])
                if record and not record['retained'] and oid == record['object']:
                    change = record['change']
                    if 'Position' in change:
                        layout.update(change['Position'])
                    elif 'Width' in change:
                        layout['max_width'] = change['Width']['points']
                        layout['width_set_by_user'] = change['Width']['user_set']
                    else:
                        self.assertEqual(node['kind']['collapse_state'], int(change['Collapsed']))
                    changed += 1
                else:
                    self.assertEqual(node['kind'].get('collapse_state'), before['kind'].get('collapse_state'))
                self.assertEqual(node['layout'], layout)
                if node['kind']['type'] == 'RichText':
                    expected = before['kind']['text']
                    if record and not record['retained'] and oid == record['dependent_text']:
                        expected = 'Local ' + expected
                        texts += 1
                    self.assertEqual(node['kind']['text'], expected)
        self.assertEqual((changed, texts), (10, 10))

    def preserved_node(self, old, new):
        expected = dict(old)
        if old['modified'] != new['modified']:
            self.assertGreater(new['modified'], old['modified'])
            expected['modified'] = new['modified']
        if old['extra'] != new['extra']:
            expected['extra'] = [old['extra'][0] + [{'id': 0x880034dd, 'value': 'NoData'}], *old['extra'][1:]]
        self.assertEqual(new, expected)

    def test_native_outline_controls_and_cold_reopen(self):
        models, captures = {}, {}
        with TemporaryDirectory() as temporary:
            for phase in ('before', 'after', 'cold'):
                folder = Path(temporary) / phase
                shutil.copytree(FIXTURE / phase / 'read', folder / 'read')
                compare(FIXTURE / phase / 'notebook', folder / 'read')
                subprocess.run([EXPORTER, FIXTURE / phase / 'notebook/synthetic.one', folder / 'model'], check=True)
                model = json.loads((folder / 'model/document.json').read_text())
                models[phase] = {r['nodes'][r['roots']['2']]['kind']['title']: (r, page)
                                 for _, _, r, page in ordered_pages(model)}
                captures[phase] = {page.get('name'): page for path in (folder / 'read').glob('page-*.xml')
                                   for page in [ET.parse(path).getroot()]}
                self.assertEqual(len(models[phase]), 15)
        cases = json.loads((FIXTURE / 'cases.json').read_text(encoding='utf-8-sig'))
        self.assertEqual(len(cases), 14)
        for case in cases:
            name = case['name']
            old, page = models['before'][name]
            outline, other = [oid for oid in old['nodes'][page]['children'] if old['nodes'][oid]['kind']['type'] == 'Outline']
            original = dict(walk(old, outline))
            target, = [oid for oid, node in original.items() if node['kind']['type'] == 'Paragraph' and node['content']
                       and old['nodes'][node['content'][0]]['kind']['text'].startswith('Target ')]
            parents = {child: oid for oid, node in original.items() for child in node['children']}
            for phase in ('after', 'cold'):
                with self.subTest(case=name, phase=phase):
                    new, current_page = models[phase][name]
                    self.assertEqual(page, current_page)
                    for oid, node in walk(old, other):
                        self.preserved_node(node, new['nodes'][oid])
                    graph = {oid: [list(node['children']), list(node['content'])] for oid, node in original.items()}
                    if name.startswith('Delete'):
                        selected = outline if name in ('Delete only paragraph', 'Delete outline') else target
                        for oid, _ in walk(old, selected):
                            del graph[oid]
                        if selected == outline:
                            self.assertNotIn(outline, new['nodes'][page]['children'])
                            active = {}
                        else:
                            graph[parents[selected]][0].remove(selected)
                            active = {oid: [node['children'], node['content']] for oid, node in walk(new, outline)}
                        self.assertEqual(active, graph)
                    elif 'keys' in case:
                        siblings = graph[outline][0]
                        if name in ('Move leaf down', 'Move subtree down'):
                            siblings[1], siblings[2] = siblings[2], siblings[1]
                        elif name == 'Move subtree up':
                            siblings[0], siblings[1] = siblings[1], siblings[0]
                        elif name == 'Indent subtree':
                            siblings.remove(target)
                            graph[siblings[0]][0] = [target]
                        elif name == 'Outdent subtree':
                            graph[parents[target]][0].remove(target)
                            siblings.insert(1, target)
                        else:
                            self.fail(name)
                        self.assertEqual({oid: [node['children'], node['content']] for oid, node in walk(new, outline)}, graph)
                        self.assertEqual(new['nodes'][target]['children'], old['nodes'][target]['children'])
                        for oid, node in original.items():
                            if node['kind']['type'] == 'RichText':
                                self.preserved_node(node, new['nodes'][oid])
                    else:
                        for oid, node in original.items():
                            if node['kind']['type'] == 'RichText':
                                continue
                            current = new['nodes'][oid]
                            self.assertEqual(current['children'], node['children'])
                            self.assertEqual(current['kind']['type'], node['kind']['type'])
                            for a, b in zip(node['content'], current['content'], strict=True):
                                self.assertEqual(old['nodes'][a]['kind']['text'], new['nodes'][b]['kind']['text'])
                        geometry = new['nodes'][outline]['layout']
                        for key, value in case.get('position', {}).items():
                            self.assertEqual(geometry[key], value)
                        if 'size' in case:
                            self.assertEqual(geometry['max_width'], case['size']['width'])
                            self.assertEqual(geometry['width_set_by_user'], case['size']['isSetByUser'])
                        if 'collapse' in case:
                            self.assertEqual(bool(new['nodes'][target]['kind']['collapse_state']), case['collapse'])
                            node, = [node for node in captures[phase][name].findall('one:Outline//one:OE', ns)
                                     if ''.join(Text(node.find('one:T', ns).text or '').parts).startswith('Target ')]
                            self.assertEqual(node.get('collapsed') in ('true', '1'), case['collapse'])
                        before_xml = captures['before'][name]
                        after_xml = captures[phase][name]
                        for left, right in zip(before_xml.findall('one:Outline', ns),
                                               sorted(after_xml.findall('one:Outline', ns), key=lambda node: int(node.find('one:Position', ns).get('z'))), strict=True):
                            for a, b in zip(native_characters(before_xml, [left]), native_characters(after_xml, [right]), strict=True):
                                for (x, before), (y, after) in zip(a, b, strict=True):
                                    self.assertEqual(x, y)
                                    for key in before.keys() | after.keys():
                                        default = 'automatic' if key in ('color', 'highlight') else False
                                        self.assertEqual(before.get(key, default), after.get(key, default))
            saved, saved_page = models['cold'][name]
            current, current_page = models['after'][name]
            self.assertEqual(current_page, saved_page)
            self.assertEqual(current['nodes'][current_page]['children'], saved['nodes'][saved_page]['children'])
            for outline in current['nodes'][current_page]['children']:
                if current['nodes'][outline]['kind']['type'] != 'Outline':
                    continue
                for oid, node in walk(current, outline):
                    for key in ('children', 'content', 'child_level'):
                        self.assertEqual(saved['nodes'][oid][key], node[key])
                    self.assertEqual(saved['nodes'][oid]['kind'].get('collapse_state'), node['kind'].get('collapse_state'))
