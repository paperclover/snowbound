import html
import json
import re
from pathlib import Path
import runpy
import shutil
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/link-edit'
compare = runpy.run_path(str(ROOT / 'tools/verify-document.py'))['compare']


class LinkEditTest(unittest.TestCase):
    def test_a_rust_hyperlink_renders_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'cold/read', read)
            compare(FIXTURE / 'candidate', read)
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')))
        texts = [oe.find('one:T', ns).text for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None]
        self.assertEqual([' '.join(text.split()) for text in texts],
                         ['Read about Rust <a href="https://example.invalid/rust">the Rust site</a>'])

    def test_onenote_links_between_pages_use_stored_identities(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'native-links/read', read)
            compare(FIXTURE / 'native-links/notebook', read)
        links = json.loads((FIXTURE / 'native-links/links.json').read_text(encoding='utf-8-sig'))
        page, = (ET.parse(path).getroot() for path in sorted((FIXTURE / 'native-links/read').glob('page-*.xml'))
                 if ET.parse(path).getroot().get('name') == 'Read about Rust the Rust site')
        hrefs = re.findall(r'href="([^"]*)"', ''.join(oe.find('one:T', ns).text for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None))
        section = re.search(r'section-id=(\{[^}]*\})', links['page']).group(1)
        page_id = re.search(r'page-id=(\{[^}]*\})', links['page']).group(1)
        self.assertEqual(hrefs[1], 'onenote:#Link%%20target&amp;section-id=%s&amp;page-id=%s&amp;end&amp;base-path=C:\\one-tests\\runs\\capture\\notebook\\links.one' % (section, page_id))
        self.assertEqual(hrefs[3], 'onenote:#section-id=%s&amp;end&amp;base-path=C:\\one-tests\\runs\\capture\\notebook\\links.one' % section)

    def test_rust_internal_links_render_natively(self):
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(FIXTURE / 'internal/cold/read', read)
            compare(FIXTURE / 'internal/candidate', read)
        pages = {ET.parse(path).getroot().get('name'): ET.parse(path).getroot() for path in sorted((FIXTURE / 'internal/cold/read').glob('page-*.xml'))}
        page = next(page for name, page in pages.items() if name.startswith('Linking page'))
        text = ' '.join(' '.join(oe.find('one:T', ns).text.split()) for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None)
        self.assertRegex(text, r'^Linking page <a href="onenote:#Link%20target&amp;section-id=\{[0-9A-F-]{36}\}&amp;page-id=\{[0-9A-F-]{36}\}&amp;end&amp;base-path=C:\\one-tests\\runs\\capture\\notebook\\links.one">Link target</a>$')

    def test_onenote_links_typed_urls_and_dialog_addresses(self):
        """OneNote 2010's read of `native-typed`: a typed `www.` address opens over http,
        and a Link dialog address typed without a scheme does too."""
        text = (FIXTURE / 'native-typed/read/page-000.xml').read_text(encoding='utf-8-sig')
        hrefs = [html.unescape(href) for href in re.findall(r'href="([^"]*)"', text)]
        self.assertEqual(hrefs[1], 'http://www.example.com')
        self.assertEqual(hrefs[11], 'http://www.d.example')
        self.assertEqual(hrefs[15], 'http://example.net/x')
        self.assertNotIn('me@example.com', hrefs)

    def test_editor_links_and_equations_render_natively(self):
        """What the canvas editor saved for typed URLs, the Link dialog, Remove Link and
        equations typed after Alt+= reads back in a cold OneNote 2010 with the links and
        MathML it meant (`crates/canvas/tests/links_equations.rs`)."""
        row = FIXTURE / 'editor'
        with TemporaryDirectory() as temporary:
            read = Path(temporary) / 'read'
            shutil.copytree(row / 'cold/read', read)
            compare(row / 'candidate', read)
        expected = json.loads((row / 'expected.json').read_text())
        path = next(path for path in sorted((row / 'cold/read').glob('page-*.xml'))
                    if ET.parse(path).getroot().get('name').startswith('Linking page'))
        page = ET.parse(path).getroot()
        text = ''.join(oe.find('one:T', ns).text for oe in page.iter('{%s}OE' % ns['one']) if oe.find('one:T', ns) is not None)
        hrefs = [html.unescape(href) for href in re.findall(r'href="([^"]*)"', text)]
        self.assertEqual(hrefs, expected['hrefs'])
        raw = path.read_text(encoding='utf-8-sig')
        mathml = [re.sub(r'&#(\d+);', lambda m: chr(int(m.group(1))), body)
                  for body in re.findall(r'<mml:math[^>]*>(.*?)</mml:math>', raw, re.S)]
        self.assertEqual(mathml, expected['mathml'])


if __name__ == '__main__':
    unittest.main()
