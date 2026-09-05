import unittest
import xml.etree.ElementTree as ET
from compare_native import content


class NativeComparisonTest(unittest.TestCase):
    def test_cache_identity_does_not_hide_text_or_formatting_changes(self):
        source = ET.fromstring('<OE objectID="cache-a" style="font-weight:bold"><T>Text</T></OE>')
        other = ET.fromstring('<OE objectID="cache-b" style="font-weight:bold"><T>Text</T></OE>')
        self.assertEqual(content(source), content(other))
        other.find('T').text = 'Different'
        self.assertNotEqual(content(source), content(other))
        other.find('T').text = 'Text'
        other.set('style', 'font-weight:normal')
        self.assertNotEqual(content(source), content(other))

    def test_geometry_and_table_order_remain_significant(self):
        source = ET.fromstring('<Table><Row><Cell>A</Cell><Cell>B</Cell></Row></Table>')
        other = ET.fromstring('<Table><Row><Cell>B</Cell><Cell>A</Cell></Row></Table>')
        self.assertNotEqual(content(source), content(other))
        first = ET.fromstring('<Position x="36" y="72"/>')
        second = ET.fromstring('<Position x="37" y="72"/>')
        self.assertNotEqual(content(first), content(second))


if __name__ == '__main__':
    unittest.main()
