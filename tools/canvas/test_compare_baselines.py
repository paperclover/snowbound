import base64
import io
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import xml.etree.ElementTree as ET

from PIL import Image

from baseline_fixture import build
from compare_baselines import baseline_report, pdf_rgb, register_images
from native_fixture import NS


class BaselineAnchors(unittest.TestCase):
    def setUp(self):
        self.xml = ET.fromstring(build()['pages'][0]['xml'])
        self.images = []
        for node in self.xml.findall(f'{{{NS}}}Image'):
            with Image.open(io.BytesIO(base64.b64decode(node.findtext(f'{{{NS}}}Data')))) as image:
                pixels, dimensions = image.convert('RGB').tobytes(), image.size
            position, size = node.find(f'{{{NS}}}Position'), node.find(f'{{{NS}}}Size')
            x, y = float(position.get('x')) + 36, float(position.get('y')) + 21.6
            self.images.append({
                'srcsize': dimensions, 'bits': 8, 'colorspace': [SimpleNamespace(name='DeviceRGB')],
                'x0': x, 'x1': x + float(size.get('width')), 'top': y, 'bottom': y + float(size.get('height')),
                'stream': SimpleNamespace(attrs={}, get_data=lambda data=pixels: data),
            })

    def test_exact_pixels_establish_translation(self):
        anchors = register_images(self.xml, [SimpleNamespace(images=self.images)])[0]
        self.assertEqual(len(anchors), 3)
        for anchor in anchors:
            self.assertAlmostEqual(anchor['translation'][0], 36)
            self.assertAlmostEqual(anchor['translation'][1], 21.6)
            self.assertEqual(anchor['extent_scale'], [1, 1])

    def test_same_dimensions_with_changed_pixels_are_not_an_anchor(self):
        self.images[0]['stream'].get_data = lambda: b'\0' * (22 * 26 * 3)
        anchors = register_images(self.xml, [SimpleNamespace(images=self.images)])[0]
        self.assertEqual(len(anchors), 2)
        self.assertTrue(all(a['source_xywh'][0] != 20.125 for a in anchors))

    def test_duplicates_and_masks_are_not_usable(self):
        self.images.append(self.images[0])
        self.images[1]['stream'].attrs['SMask'] = object()
        anchors = register_images(self.xml, [SimpleNamespace(images=self.images)])[0]
        self.assertEqual(len(anchors), 1)
        self.assertEqual(anchors[0]['source_xywh'][0], 25.75)

    def test_indexed_four_bit_pixels_respect_row_padding(self):
        colors = b'\xff\0\0\0\xff\0\0\0\xff'
        image = {'srcsize': (3, 2), 'bits': 4,
                 'colorspace': [SimpleNamespace(name='Indexed'), SimpleNamespace(name='DeviceRGB'), 2, colors],
                 'stream': SimpleNamespace(attrs={}, get_data=lambda: b'\x01\x20\x01\x20')}
        self.assertEqual(pdf_rgb(image), colors * 2)

    def test_text_geometry_cannot_change_image_registration(self):
        probe = {'objects': [{'kind': 'outline', 'id': 'o', 'is_title': False, 'layout': {'y': 400}}]}
        wraps = {'counts': {'matched': 1}, 'paragraphs': [{
            'status': 'matched', 'outline_id': 'o', 'paragraph_id': 'p', 'canvas_baselines_in_outline': [10],
            'native_instances': [{'pdf_page': 1, 'baseline_ranges_from_pdf_top': [[430, 430]]}],
        }]}
        with patch('compare_baselines.compare', return_value=wraps):
            before = baseline_report(probe, self.xml, [SimpleNamespace(images=self.images)])
            probe['objects'][0]['layout']['y'] += 20
            after = baseline_report(probe, self.xml, [SimpleNamespace(images=self.images)])
        self.assertEqual(before['anchors_by_pdf_page'], after['anchors_by_pdf_page'])
        self.assertAlmostEqual(before['paragraphs'][0]['lines'][0]['residual_using_median_anchor_translation'][0], -1.6)
        self.assertAlmostEqual(after['paragraphs'][0]['lines'][0]['residual_using_median_anchor_translation'][0], -21.6)

    def test_wrap_mismatch_remains_visible_without_baseline_measurements(self):
        wraps = {'counts': {'different_wraps': 1}, 'paragraphs': [{'status': 'different_wraps', 'paragraph_id': 'p'}]}
        with patch('compare_baselines.compare', return_value=wraps):
            report = baseline_report({'objects': []}, self.xml, [SimpleNamespace(images=self.images)])
        self.assertEqual(report['paragraphs'], [{'status': 'different_wraps', 'paragraph_id': 'p', 'lines': []}])


if __name__ == '__main__':
    unittest.main()
