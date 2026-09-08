import json
from pathlib import Path
import runpy
import shutil
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
                            fields = {field['id'] for field in new['nodes'][outline]['extra'][0]}
                            self.assertEqual(0x88001cbd in fields, case['size']['isSetByUser'])
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
