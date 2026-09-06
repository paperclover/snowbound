import hashlib
import json
from pathlib import Path
import tempfile
import unittest
import xml.etree.ElementTree as ET

from native_xml import ns
from verify_smb_faults import verify


class NativeFaultOracle(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.cases = self.root / 'cases'
        self.worker = self.root / 'captures/worker-0'
        self.worker.mkdir(parents=True)
        self.records = [{'case': 'sample-000', 'visible': 'before'},
                        {'case': 'sample-001', 'visible': 'after'}]
        self.results = []
        for role, names in [('source', ['sample']), ('interrupted', ['sample-000', 'sample-001']),
                            ('recovered', ['sample-000', 'sample-001'])]:
            (self.cases / role).mkdir(parents=True)
            for name in names:
                content = (role + name).encode()
                (self.cases / role / (name + '.one')).write_bytes(content)
                name = role + '-' + name
                self.results.append({'name': name, 'error': None, 'pages': 1,
                                     'source_sha256': hashlib.sha256(content).hexdigest()})
                replacement = 'before'
                if name == 'interrupted-sample-001': replacement = 'after'
                if role == 'recovered': replacement = 'after again'
                self.page(name, ['Title', replacement, 'Unrelated café 🦀'])
        (self.cases / 'sample-intent.json').write_text(json.dumps({'before': 'before', 'after': 'after', 'suffix': ' again'}))
        self.save()

    def save(self):
        (self.cases / 'results.json').write_text(json.dumps(self.records))
        (self.worker / 'results.json').write_text(json.dumps(self.results))

    def page(self, name, paragraphs):
        page = ET.Element('{' + ns['one'] + '}Page')
        outline = ET.SubElement(page, '{' + ns['one'] + '}Outline')
        for text in paragraphs:
            ET.SubElement(outline, '{' + ns['one'] + '}T').text = text
        output = self.worker / 'results' / name
        output.mkdir(parents=True, exist_ok=True)
        ET.ElementTree(page).write(output / 'page-0.xml', encoding='utf-8')

    def check(self):
        return verify(self.root, self.root / 'captures')

    def test_complete_before_and_after_outcomes(self):
        self.assertEqual(self.check(), {'cases': 2, 'cold_native_opens': 5, 'exact_native_text': True})

    def test_unrelated_or_partial_edits_are_rejected(self):
        for paragraphs in [['Title', 'after again', 'Changed'], ['Title', 'afte', 'Unrelated café 🦀'],
                           ['Title', 'after again again', 'Unrelated café 🦀']]:
            with self.subTest(paragraphs=paragraphs):
                self.page('recovered-sample-000', paragraphs)
                with self.assertRaises(AssertionError): self.check()

    def test_wrong_publication_outcome_is_rejected(self):
        self.records[0]['visible'] = 'after'
        self.save()
        with self.assertRaises(AssertionError): self.check()

    def test_input_hash_is_checked(self):
        (self.cases / 'interrupted/sample-000.one').write_bytes(b'changed')
        with self.assertRaises(AssertionError): self.check()

    def test_missing_and_duplicate_captures_are_rejected(self):
        original = self.results[:]
        for rows in [original[:-1], original + original[:1]]:
            self.results = rows
            self.save()
            with self.assertRaises(AssertionError): self.check()

    def test_extra_artifacts_are_rejected(self):
        (self.cases / 'interrupted/extra.one').write_bytes(b'extra')
        with self.assertRaises(AssertionError): self.check()

    def test_duplicate_cases_are_rejected(self):
        self.records.append(self.records[0])
        self.save()
        with self.assertRaises(AssertionError): self.check()

    def test_native_failure_and_wrong_page_count_are_rejected(self):
        for error, pages in [('Open failed', 1), (None, 0), (None, 2)]:
            self.results[0].update(error=error, pages=pages)
            self.save()
            with self.assertRaises(AssertionError): self.check()


if __name__ == '__main__': unittest.main()
