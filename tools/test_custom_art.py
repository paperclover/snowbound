from pathlib import Path
import hashlib
import json
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/custom-art'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


def tagged(read):
    """The page's tagged paragraphs: text, then each tag as (name, symbol, completed)."""
    page = ET.parse(read / 'page-000.xml').getroot()
    definitions = {d.get('index'): (d.get('name'), int(d.get('symbol'))) for d in page.findall('one:TagDef', ns)}
    return [((oe.find('one:T', ns).text or ''),
             [definitions[tag.get('index')] + (tag.get('completed'),) for tag in oe.findall('one:Tag', ns)])
            for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:Tag', ns) is not None]


class CustomArtTest(unittest.TestCase):
    def test_tags_with_art_cold_open_with_their_fallback_symbols(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/before-read', read)
            compare(FIXTURE / 'candidate', read)
        self.assertEqual(tagged(FIXTURE / 'cold/before-read'), [
            ('Launch day', [('Launch', 13, 'true')]),
            ('Ship the release', [('Ship it', 127, 'true')]),
            ('Checked off', [('Done', 3, 'true')]),
            ('Plain OneNote tag', [('Important', 13, 'true')]),
        ])

    def test_onenote_editing_the_page_keeps_the_mapping_and_the_folder(self):
        edited = tagged(FIXTURE / 'cold/read')
        self.assertEqual([text for text, _ in edited][:1], ['Launch day, moved to Friday'])
        self.assertEqual(edited[2][1], [('Done', 3, 'false')])
        # Every tag the mapping names is still on the page under its name and symbol.
        sidecar = FIXTURE / 'candidate/.snowbound'
        mapped = {(m['name'], m['shape']): m['art'] for m in json.loads((sidecar / 'tags.json').read_text())}
        stored = {tag[:2] for _, tags in edited for tag in tags}
        self.assertEqual(set(mapped) - stored, set())
        for art in mapped.values():
            self.assertEqual(hashlib.sha256((sidecar / 'tags' / art).read_bytes()).hexdigest(), art.split('.')[0])
        # OneNote kept the folder hidden, left no table of contents in it and changed none of it.
        left = json.loads((FIXTURE / 'cold/sidecar.json').read_text(encoding='utf-8-sig'))
        self.assertIn('Hidden', left['attributes'])
        files = {f['path']: f['sha256'] for f in left['files'] if f['sha256']}
        expected = {p.relative_to(sidecar).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
                    for p in sidecar.rglob('*') if p.is_file()}
        self.assertEqual(files, expected)
        self.assertEqual(sorted(left['notebook']), ['.snowbound', 'Custom art.one', 'Open Notebook.onetoc2'])


if __name__ == '__main__':
    unittest.main()
