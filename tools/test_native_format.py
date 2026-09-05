import unittest
import xml.etree.ElementTree as ET
from native_format import compare_formats, native_characters, Runs
from native_xml import ns, Text, project_text


class FormatOracleTest(unittest.TestCase):
    def test_native_omits_exactly_one_final_paragraph_break(self):
        for stored, exported in [('A\r\r', 'A\n'), ('\rA\r', '\nA'), ('\r', ''), ('\r\r', '\n'), ('A\n', 'A\n')]:
            with self.subTest(stored=stored):
                node = {'format': {'bold': True}, 'kind': {'text': stored, 'paragraph_style': None,
                        'runs': [{'start': 0, 'end': len(stored), 'format': None}]}}
                native = [[(c, {'bold': True}) for c in exported]]
                self.assertEqual(compare_formats({'nodes': {}}, [node], native), (len(exported), []))
                with self.assertRaisesRegex(AssertionError, 'Visible text runs differ'):
                    compare_formats({'nodes': {}}, [node], [[(c, {'bold': True}) for c in exported + '\n']])

    def test_native_break_formatting_and_tabs_preserve_distinct_source_text(self):
        html = '<b>A<br><br>\nB</b><br>\nC&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;D'
        expected = 'A\n\nB\nC' + '\u00a0' * 8 + 'D'
        self.assertEqual(''.join(Text(html).parts), expected)
        runs = Runs(html, {})
        self.assertEqual(''.join(c for c, _ in runs.characters), expected)
        self.assertTrue(runs.characters[2][1]['bold'])
        self.assertNotIn('bold', runs.characters[4][1])
        self.assertEqual(project_text('A\r\rB\rC\tD'), expected)
        self.assertEqual(project_text('\u00a0' * 8), '\u00a0' * 8)

    def test_utf16_run_boundaries_and_inherited_native_style(self):
        page = ET.fromstring('''<one:Page xmlns:one="http://schemas.microsoft.com/office/onenote/2010/onenote">
          <one:QuickStyleDef index="0" font="Calibri" fontSize="14" bold="true" strikethrough="true"/>
          <one:Outline><one:OE quickStyleIndex="0"><one:T>A&lt;span style="font-style:italic"&gt;😀&lt;/span&gt;B</one:T></one:OE></one:Outline>
        </one:Page>''')
        node = {'format': {}, 'kind': {'text': 'A😀B', 'paragraph_style': 'base', 'runs': [
            {'start': 0, 'end': 1, 'format': None}, {'start': 1, 'end': 3, 'format': 'italic'},
            {'start': 3, 'end': 4, 'format': None},
        ]}}
        space = {'nodes': {'base': {'format': {'bold': True, 'font': 'Calibri', 'font_size': 14.0}},
                           'italic': {'format': {'italic': True}}}}
        expected = native_characters(page, page.findall('one:Outline', ns))
        self.assertTrue(expected[0][0][1]['strike'])
        count, differences = compare_formats(space, [node], expected)
        self.assertEqual((count, differences), (10, []))
        space['nodes']['italic']['format']['italic'] = False
        _, differences = compare_formats(space, [node], expected)
        self.assertEqual(differences, [{'paragraph': 0, 'field': 'italic', 'stored': False,
                                       'native': True, 'characters': 1, 'positions': [1]}])

    def test_partial_black_omission_is_reported_not_normalized_away(self):
        node = {'format': {}, 'kind': {'text': 'ABA', 'paragraph_style': None, 'runs': [
            {'start': 0, 'end': 1, 'format': None}, {'start': 1, 'end': 2, 'format': 'black'},
            {'start': 2, 'end': 3, 'format': None},
        ]}}
        space = {'nodes': {'black': {'format': {'highlight': 0}}}}
        _, differences = compare_formats(space, [node], [[(c, {'highlight': 'automatic'}) for c in 'ABA']])
        self.assertEqual(differences, [{'paragraph': 0, 'field': 'highlight', 'stored': '#000000',
                                       'native': 'automatic', 'characters': 1, 'positions': [1]}])


if __name__ == '__main__':
    unittest.main()
