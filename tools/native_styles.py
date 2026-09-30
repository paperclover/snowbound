"""Cold-open a notebook of themed pages in OneNote 2010 and capture what it draws and
reports of each page's styles.

Usage: native_styles.py NOTEBOOK_DIR OUTPUT_DIR
"""
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=4, author=Path(__file__).resolve().parent / 'native/styles.ps1',
                   screenshots=True, collect_notebook=True)
