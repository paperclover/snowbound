import unittest

import numpy as np

from compare_scroll import compare


class ScrollTests(unittest.TestCase):
    def test_overlap_ignores_blank_rows_and_survives_a_changed_caret(self):
        page = np.full((120, 20, 3), 255, dtype=np.uint8)
        for y in range(0, len(page), 3):
            page[y, 2:8] = [y, 40, 80]
        before, after = page[:90].copy(), page[30:].copy()
        after[20:25, 12] = 0
        result = compare(before, after, 20)
        self.assertEqual(result['scroll_y'], 30)
        self.assertFalse(result['ambiguous'])
        self.assertGreater(result['candidates'][0]['foreground_equal_fraction'], 0.95)

    def test_repeated_foreground_has_no_unique_offset(self):
        image = np.full((40, 20, 3), 255, dtype=np.uint8)
        image[::4, 2:8] = 0
        result = compare(image, image, 20)
        self.assertTrue(result['ambiguous'])
        self.assertIsNone(result['scroll_y'])

    def test_blank_or_unrelated_images_cannot_establish_scroll(self):
        white = np.full((40, 20, 3), 255, dtype=np.uint8)
        with self.assertRaises(ValueError):
            compare(white, white, 20)
        before, after = white.copy(), white.copy()
        before[:, 2:8] = [30, 40, 80]
        after[:, 2:8] = [31, 40, 80]
        with self.assertRaises(ValueError):
            compare(before, after, 20)
        with self.assertRaises(ValueError):
            compare(before, after[:-1], 20)


if __name__ == '__main__':
    unittest.main()
