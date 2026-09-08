import base64
from io import BytesIO
import json
import hashlib
from pathlib import Path
import re
from tempfile import TemporaryDirectory
import shutil
import subprocess
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from PIL import Image
from notebook_report import generate
from document_model import EXPORTER


class NotebookReportTest(unittest.TestCase):
    def test_external_assets_match_native_bytes_and_refresh_without_a_section_edit(self):
        from notebook_editor import Session
        fixture = Path(__file__).resolve().parent.parent / 'corpus/native-external-assets'
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'source'
            shutil.copytree(fixture / 'notebook', source)
            session = Session(source, root / 'session')
            first = root / 'session/g/0/report'
            section = next(p.parent for p in (first / 'model').glob('*/assets.json')
                           if json.loads(p.read_text()))
            rows = json.loads((section / 'assets.json').read_text())
            expected = {row['sha256'] for row in json.loads((fixture / 'read/payloads.json').read_text(encoding='utf-8-sig'))}
            actual = {hashlib.sha256((section / row['path']).read_bytes()).hexdigest() for row in rows}
            self.assertTrue(expected <= actual)
            for payload in source.rglob('*.onebin'):
                self.assertEqual(payload.read_bytes(), (root / 'session/g/0/snapshot' / payload.relative_to(source)).read_bytes())
            payload, = [p for p in source.rglob('*.onebin') if p.stat().st_size == 1024]
            reference = {'External': payload.name}
            old, = [row for row in rows if row['reference'] == reference]
            previous_bytes = (section / old['path']).read_bytes()
            payload.unlink()
            generate(source, root / 'missing', previous=first)
            missing, = [row for p in (root / 'missing/model').glob('*/assets.json')
                        for row in json.loads(p.read_text()) if row['reference'] == reference]
            self.assertIsNone(missing['path'])
            self.assertEqual(missing['error']['kind'], 'NotFound')
            payload.write_bytes(b'Changed external payload')
            generate(source, root / 'changed', previous=root / 'missing')
            changed, = [(p.parent, row) for p in (root / 'changed/model').glob('*/assets.json')
                        for row in json.loads(p.read_text()) if row['reference'] == reference]
            self.assertEqual((changed[0] / changed[1]['path']).read_bytes(), payload.read_bytes())
            self.assertEqual((section / old['path']).read_bytes(), previous_bytes)

    def test_locked_and_unreadable_sections_remain_visible_in_cached_reports(self):
        fixture = Path(__file__).resolve().parent.parent / 'corpus/native-encrypted/cold-encrypted-02/notebook'
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            generate(fixture, root / 'first')
            generate(fixture, root / 'cached', previous=root / 'first')
            index = (root / 'first/index.html').read_text()
            self.assertIn('2 sections', index)
            self.assertIn('Locked section', index)
            self.assertIn('Cannot read Open Notebook.one', index)
            self.assertIn('Object space has no revision manifest list', index)
            self.assertEqual(json.loads((root / 'first/pages.json').read_text()), [])
            self.assertEqual(index, (root / 'cached/index.html').read_text())

    def test_tiff_preview_matches_native_pixels_and_retains_original_bytes(self):
        fixture = Path(__file__).resolve().parent.parent / 'corpus/m6/native-features-01'
        with TemporaryDirectory() as temporary:
            output = Path(temporary) / 'report'
            generate(fixture / 'notebook', output)
            page, = [p for p in json.loads((output / 'pages.json').read_text()) if p['title'] == 'Image tiff']
            source, = re.findall(r'<img[^>]*src="([^"]+)"', (output / page['report']).read_text())
            preview = output / source
            self.assertEqual(preview.suffix, '.png')
            original, = (output / 'assets').glob('*.tiff')
            self.assertEqual(original.read_bytes(), (fixture / 'notebook/fixture-data/pattern.tiff').read_bytes())
            native, = [root for p in (fixture / 'read').glob('page-*.xml')
                       if (root := ET.parse(p).getroot()).get('name') == 'Image tiff']
            data, = [n.text for n in native.iter() if n.tag.endswith('}Data')]
            with Image.open(BytesIO(base64.b64decode(data))) as expected, Image.open(preview) as actual:
                self.assertEqual(actual.size, expected.size)
                self.assertEqual(actual.convert('RGBA').tobytes(), expected.convert('RGBA').tobytes())
            references = [a['path'] for p in (output / 'model').glob('*/assets.json') for a in json.loads(p.read_text())]
            self.assertIn('../../assets/' + original.name, references)

    def test_reused_models_and_assets_match_a_fresh_report_after_source_order_changes(self):
        fixture = Path(__file__).resolve().parent.parent / 'corpus/m6/native-features-01/notebook'
        with TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / 'notebook'
            shutil.copytree(fixture, source)
            generate(source, root / 'first')
            before = {p.relative_to(root / 'first'): p.read_bytes() for p in (root / 'first').rglob('*') if p.is_file()}
            def cached_run(command, *args, **kwargs):
                self.assertNotEqual(command[0], EXPORTER, 'Unchanged model exported again')
                return original(command, *args, **kwargs)

            original = subprocess.run
            with patch('notebook_report.subprocess.run', side_effect=cached_run):
                generate(source, root / 'same', previous=root / 'first')
            self.assertEqual(before, {p.relative_to(root / 'same'): p.read_bytes() for p in (root / 'same').rglob('*') if p.is_file()})
            subprocess.run([EXPORTER.parent / 'create_section', source / 'a.one', 'Independent section', 'Fixture'], check=True)
            with patch('notebook_report.subprocess.run', wraps=original) as run:
                generate(source, root / 'changed', previous=root / 'first')
            self.assertEqual([Path(call.args[0][1]).name for call in run.call_args_list if call.args[0][0] == EXPORTER], ['a.one'])
            generate(source, root / 'fresh')
            self.assertEqual({p.relative_to(root / 'fresh'): p.read_bytes() for p in (root / 'fresh').rglob('*') if p.is_file()},
                             {p.relative_to(root / 'changed'): p.read_bytes() for p in (root / 'changed').rglob('*') if p.is_file()})
            for name, contents in before.items():
                self.assertEqual((root / 'first' / name).read_bytes(), contents)
            old_index = next(i for i, row in enumerate(json.loads((root / 'first/source.json').read_text())) if row['path'] == 'synthetic.one')
            new_index = next(i for i, row in enumerate(json.loads((root / 'changed/source.json').read_text())) if row['path'] == 'synthetic.one')
            self.assertEqual((root / f'first/model/{old_index}/document.json').stat().st_ino,
                             (root / f'changed/model/{new_index}/document.json').stat().st_ino)
            (source / 'a.one').unlink()
            with patch('notebook_report.subprocess.run', side_effect=cached_run):
                generate(source, root / 'deleted', previous=root / 'changed')
            self.assertEqual(before, {p.relative_to(root / 'deleted'): p.read_bytes() for p in (root / 'deleted').rglob('*') if p.is_file()})


if __name__ == '__main__':
    unittest.main()
