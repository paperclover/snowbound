from pathlib import Path
import runpy
import unittest
from types import SimpleNamespace
from unittest.mock import patch

verify = runpy.run_path(str(Path(__file__).with_name('verify-document.py')))['verify_pdf_black']


class PdfFormatOracleTest(unittest.TestCase):
    def test_unmapped_text_cannot_hide_missing_text_or_incorrect_highlighting(self):
        chars = [{'text': c, 'x0': i * 2, 'x1': i * 2 + 1, 'top': 0, 'bottom': 1}
                 for i, c in enumerate('BeforeSilk after  tail') if not c.isspace()]
        rect = {'fill': True, 'non_stroking_color': 0, 'x0': 12, 'x1': 19, 'top': 0, 'bottom': 1}
        page = SimpleNamespace(page_number=1, chars=chars, rects=[rect])
        runs = [{'text': text, 'format': {'hidden': False, 'highlight': highlight}}
                for text, highlight in [('Before', None), ('Silk', 0), (' after e\u0301 tail', None)]]
        with patch('pdfplumber.open') as opened:
            opened.return_value.__enter__.return_value.pages = [page]
            self.assertEqual(verify('unused', [runs]), 1)
            page.rects.append({**rect, 'x0': 32, 'x1': 35})
            with self.assertRaisesRegex(AssertionError, 'unhighlighted unmapped text'):
                verify('unused', [runs])
            page.rects.pop()
            runs[2]['text'] = ' after ordinary tail'
            with self.assertRaisesRegex(AssertionError, 'ambiguous'):
                verify('unused', [runs])
            runs[2].update(text=' after e\u0301 tail', format={'hidden': False, 'highlight': 0})
            with self.assertRaisesRegex(AssertionError, 'visual verification'):
                verify('unused', [runs])
            runs[2]['format']['highlight'] = None
            page.chars += chars
            with self.assertRaisesRegex(AssertionError, 'ambiguous'):
                verify('unused', [runs])
            page.chars = [{**chars[0], 'text': 'a', 'x0': i * 2, 'x1': i * 2 + 1} for i in range(3)]
            page.rects = [{**rect, 'x0': 0, 'x1': 5}]
            with self.assertRaisesRegex(AssertionError, 'ambiguous'):
                verify('unused', [[{'text': 'aa', 'format': {'hidden': False, 'highlight': 0}}]])

    def test_native_pdf_locates_partial_black_by_paragraph_and_character(self):
        pdf = Path(__file__).resolve().parent.parent / 'corpus/m6/native-structure-01/read/page-001.pdf'
        runs = [{'text': text, 'format': {'hidden': False, 'highlight': highlight}}
                for text, highlight in [('Before ', None), ('middle', 0), (' after', None)]]
        self.assertEqual(verify(pdf, [runs]), 1)
        runs[0]['text'] = 'Before m'
        runs[1]['text'] = 'iddle '
        runs[2]['text'] = 'after'
        with self.assertRaisesRegex(AssertionError, 'character positions'):
            verify(pdf, [runs])


if __name__ == '__main__':
    unittest.main()
