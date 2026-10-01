"""Password-protected sections both ways: OneNote 2010 cold-opens and unlocks sections
Snowbound protected, edited, gave another password, unprotected and merged, and Snowbound
reads what OneNote wrote, protecting, changing and removing passwords and typing into them
(`corpus/protected-sections`)."""
import json
from pathlib import Path
import re
import subprocess
from tempfile import TemporaryDirectory
import unittest
import xml.etree.ElementTree as ET

from document_model import EXPORTER

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / 'corpus/protected-sections'
ONE = '{http://schemas.microsoft.com/office/onenote/2010/onenote}'


def runs(value):
    if isinstance(value, dict):
        own = [value['text']] if isinstance(value.get('text'), str) else []
        return own + [text for item in value.values() if isinstance(item, (dict, list)) for text in runs(item)]
    if isinstance(value, list):
        return [text for item in value for text in runs(item)]
    return []


def plain(text):
    """A native `T` element's text without its HTML spans."""
    return re.sub(r'<[^>]+>', '', text).replace('\n', ' ').strip()


def squeezed(text):
    return re.sub(r'\s', '', text)


def clear(text, stored):
    """Whether `stored` holds `text` in the clear, as OneNote stores text: UTF-16 or ASCII."""
    return text.encode('utf-16-le') in stored or text.encode() in stored


class ProtectedSectionsTest(unittest.TestCase):
    def setUp(self):
        self.temporary = TemporaryDirectory()
        self.root = Path(self.temporary.name)

    def tearDown(self):
        self.temporary.cleanup()

    def export(self, section, password):
        """The text Snowbound reads from `section`, under `password` when it has one."""
        output = self.root / f'export-{len(list(self.root.iterdir()))}'
        command = [EXPORTER, section, output]
        if password is not None:
            file = self.root / f'password-{len(list(self.root.iterdir()))}'
            file.write_text(password)
            command += ['--password-file', file]
        subprocess.run(command, check=True, capture_output=True)
        return ''.join(runs(json.loads((output / 'text.json').read_text())))

    def native_pages(self):
        """The cold read's paragraph texts by section name."""
        hierarchy = ET.parse(FIXTURE / 'cold/read/hierarchy.xml').getroot()
        sections, encrypted = {}, set()
        for section in hierarchy.iter(ONE + 'Section'):
            if section.get('encrypted') == 'true':
                encrypted.add(section.get('name'))
            for page in section.iter(ONE + 'Page'):
                sections[page.get('ID')] = section.get('name')
        found = {}
        for path in sorted((FIXTURE / 'cold/read').glob('page-*.xml')):
            page = ET.parse(path).getroot()
            name = sections[page.get('ID')]
            found.setdefault(name, []).extend(
                plain(element.text) for element in page.iter(ONE + 'T') if element.text and plain(element.text))
        return found, encrypted

    def test_onenote_unlocks_and_reads_every_section_snowbound_protected(self):
        passwords = json.loads((FIXTURE / 'candidate/passwords.json').read_text())
        found, encrypted = self.native_pages()
        self.assertEqual(encrypted, {Path(name).stem for name, password in passwords.items() if password})
        expected = {
            'Edited': ['Edited first under the key. Edited second under the key. Edited third under the key.'],
            'Changed': ['Written under the changed password.'],
            'Conflicted': ['Ours offline.'],
        }
        for name, password in passwords.items():
            stem = Path(name).stem
            native = ' '.join(found[stem])
            self.assertIn('Fictitious positioned outline.', native, stem)
            for text in expected.get(stem, []):
                self.assertIn(text, native, stem)
            stored = (FIXTURE / 'candidate/notebook' / name).read_bytes()
            for text in expected.get(stem, []) + ['Fictitious positioned outline.']:
                self.assertEqual(clear(text, stored), password is None, stem)
            # Snowbound's own read of the candidate holds every paragraph OneNote read but the
            # one OneNote typed after.
            read = self.export(FIXTURE / 'candidate/notebook' / name, password)
            for paragraph in found[stem]:
                if paragraph != 'Typed by OneNote 2010 under the key.':
                    self.assertIn(squeezed(paragraph), squeezed(read), stem)

    def test_snowbound_reads_what_onenote_wrote_into_its_protected_sections(self):
        passwords = json.loads((FIXTURE / 'candidate/passwords.json').read_text())
        for name, password in passwords.items():
            read = self.export(FIXTURE / 'cold/notebook' / name, password)
            self.assertIn('Fictitiouspositionedoutline.', squeezed(read), name)
            if name == 'Edited.one':
                self.assertIn('TypedbyOneNote2010underthekey.', squeezed(read))
                self.assertIn('Editedthirdunderthekey.', squeezed(read))
        wrong = subprocess.run([EXPORTER, FIXTURE / 'cold/notebook/Edited.one', '--password-file',
                                self.write('Protected gate password')], capture_output=True)
        self.assertNotEqual(wrong.returncode, 0)

    def write(self, text):
        file = self.root / 'wrong'
        file.write_text(text)
        return file

    def test_onenote_protected_changed_and_removed_sections_read_back(self):
        manifest = json.loads((FIXTURE / 'manifest.json').read_text())
        for name, password in manifest['native'].items():
            read = self.export(FIXTURE / 'native' / name, password)
            self.assertIn('Secretwords:secretwordhidden', squeezed(read), name)
            stored = (FIXTURE / 'native' / name).read_bytes()
            self.assertEqual(clear('secretword', stored), password is None, name)


if __name__ == '__main__':
    unittest.main()
