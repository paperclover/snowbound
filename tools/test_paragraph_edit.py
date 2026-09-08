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
from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/paragraph-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class ParagraphEditTest(unittest.TestCase):
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
