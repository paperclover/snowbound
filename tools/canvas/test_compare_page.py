import unittest
import xml.etree.ElementTree as ET

from compare_page import compare


class PageComparisonTests(unittest.TestCase):
    def test_matches_reordered_outlines_and_preserves_html_whitespace(self):
        native = ET.fromstring('''<Page xmlns="http://schemas.microsoft.com/office/onenote/2010/onenote" name="Page">
          <Outline objectID="second"><Position x="70" y="30"/><Size width="90" height="14"/>
            <OEChildren><OE><T>B</T></OE></OEChildren></Outline>
          <Outline objectID="first"><Position x="36" y="20"/><Size width="100" height="28"/>
            <OEChildren><OE><T>&lt;b&gt;A&amp;nbsp;&lt;/b&gt;&lt;br/&gt;B</T></OE><OE><T/></OE></OEChildren></Outline>
        </Page>''')
        probe = {'title': 'Page', 'margin_origin': [36.5, 14.4], 'measurement': 'Raw paragraphs',
                 'objects': [
                     {'kind': 'outline', 'is_title': False, 'unsupported': [], 'id': 'a',
                      'layout': {'x': 36.5, 'y': 20, 'max_width': 100},
                      'paragraphs': [{'visible_text': text, 'unsupported': [], 'height': 14,
                                      'parent': None, 'lists': []} for text in ['A\u00a0\nB', '']]},
                     {'kind': 'outline', 'is_title': False, 'unsupported': [], 'id': 'b',
                      'layout': {'x': 70.5, 'y': 30, 'max_width': 90},
                      'paragraphs': [{'visible_text': 'B', 'unsupported': [], 'height': 14,
                                      'parent': None, 'lists': []}]},
                 ]}
        result = compare(probe, native)
        self.assertEqual([o['native_id'] for o in result['outlines']], ['first', 'second'])
        self.assertEqual(result['outlines'][0]['height_residual'], 0)
        self.assertEqual(result['outlines'][0]['inferred_native_margin_origin'], [36, 14.4])
        probe['objects'][0]['paragraphs'][0]['visible_text'] = 'A \nB'
        with self.assertRaises(ValueError):
            compare(probe, native)

    def test_native_empty_nbsp_requires_explicit_normalization_and_is_reported(self):
        native = ET.fromstring('''<Page xmlns="http://schemas.microsoft.com/office/onenote/2010/onenote" name="Page">
          <Outline><Position x="0" y="0"/><Size width="100" height="14"/>
            <OEChildren><OE><T/></OE></OEChildren></Outline>
        </Page>''')
        probe = {'title': 'Page', 'margin_origin': [0, 0], 'measurement': 'Raw paragraphs', 'objects': [
            {'kind': 'outline', 'is_title': False, 'unsupported': [], 'id': 'a',
             'layout': {'x': 0, 'y': 0, 'max_width': 100},
             'paragraphs': [{'visible_text': '\u00a0', 'unsupported': [], 'height': 14,
                             'parent': None, 'lists': []}]}]}
        with self.assertRaises(ValueError):
            compare(probe, native)
        result = compare(probe, native, allow_native_empty_nbsp=True)['outlines'][0]
        self.assertFalse(result['text_matches_exactly'])
        self.assertEqual(result['native_empty_nbsp_paragraphs'], [0])

    def test_rejects_ambiguous_identical_content(self):
        native = ET.fromstring('''<Page xmlns="http://schemas.microsoft.com/office/onenote/2010/onenote" name="Page">
          <Outline><OEChildren><OE><T>same</T></OE></OEChildren></Outline>
          <Outline><OEChildren><OE><T>same</T></OE></OEChildren></Outline>
        </Page>''')
        probe = {'title': 'Page', 'objects': [
            {'kind': 'outline', 'is_title': False, 'unsupported': [], 'id': 'a',
             'paragraphs': [{'visible_text': 'same', 'unsupported': []}]}]}
        with self.assertRaises(ValueError):
            compare(probe, native)


if __name__ == '__main__':
    unittest.main()
