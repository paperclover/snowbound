import base64
from io import BytesIO
import json
from pathlib import Path
import re
from tempfile import TemporaryDirectory
import shutil
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from PIL import Image
from notebook_report import generate


class NotebookReportTest(unittest.TestCase):
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
            with patch('notebook_report.subprocess.run', side_effect=AssertionError('Unchanged model exported again')):
                generate(source, root / 'same', previous=root / 'first')
            self.assertEqual(before, {p.relative_to(root / 'same'): p.read_bytes() for p in (root / 'same').rglob('*') if p.is_file()})
            shutil.copyfile(source / 'synthetic.one', source / 'a.one')
            import notebook_report
            original = notebook_report.subprocess.run
            with patch('notebook_report.subprocess.run', wraps=original) as run:
                generate(source, root / 'changed', previous=root / 'first')
            self.assertEqual([Path(call.args[0][1]).name for call in run.call_args_list], ['a.one'])
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
            with patch('notebook_report.subprocess.run', side_effect=AssertionError('Unchanged model exported again')):
                generate(source, root / 'deleted', previous=root / 'changed')
            self.assertEqual(before, {p.relative_to(root / 'deleted'): p.read_bytes() for p in (root / 'deleted').rglob('*') if p.is_file()})


if __name__ == '__main__':
    unittest.main()
