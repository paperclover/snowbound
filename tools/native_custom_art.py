"""Cold-open a notebook whose tags carry Snowbound's art in OneNote 2010, have OneNote edit
the tagged page, and capture what it shows, stores and leaves of the `.snowbound` folder.

Usage: native_custom_art.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows
EDIT = Path(__file__).resolve().parent / 'native/custom-art-edit.ps1'


def interaction(name, output):
    (output / 'scripts' / EDIT.name).write_bytes(EDIT.read_bytes())
    result = windows.do_put(EDIT, 'C:\\one-tests\\custom-art-edit.ps1', name)
    if result.get('error'):
        raise RuntimeError(result['error'])
    runner.command(name, 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\custom-art-edit.ps1 -Root C:\\one-tests\\runs\\capture', output, 300000)
    page, = sorted((output / 'read').glob('page-*.xml'))
    result = windows.do_exec(runner.navigation_script(ET.parse(page).getroot().attrib['ID']), target=name, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / 'edited.png').write_bytes(base64.b64decode(screenshot))
    (output / 'edited.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=1, author=Path(__file__).resolve().parent / 'native/custom-art.ps1',
                   screenshots=True, collect_notebook=True, interaction=interaction)
