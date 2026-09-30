"""Type beside a file and a picture alone in their outlines in OneNote 2010 and capture what it
stores. The oracle for the canvas editor's provisional paragraph after an outline's objects.

Usage: native_object_outline.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner
from native_xml import ns

windows = runner.windows

# Where a click lands beside each page's object, inside its outline (at 72, 108 pt).
CLICKS = {'File alone': (236, 301), 'Picture alone': (238, 282)}


def interaction(name, output):
    pages = {}
    for path in sorted((output / 'read').glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        if title is not None and title.text in CLICKS:
            pages[title.text] = page.attrib['ID']
    for title, (x, y) in CLICKS.items():
        script = '''ComObject("OneNote.Application").NavigateTo("%s", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
CoordMode("Mouse", "Screen")
MouseMove(190, 260)
Sleep(800)
Click(%d, %d)
Sleep(500)
SendText("abc")
Sleep(2000)
''' % (pages[title], x, y)
        stem = title.lower().replace(' ', '-')
        (output / (stem + '.ahk')).write_text(script)
        result = windows.do_exec(script, target=name, timeout_ms=120000, shot_delay_ms=1500)
        screenshot = result.pop('png_b64', None)
        if screenshot:
            (output / (stem + '.png')).write_bytes(base64.b64decode(screenshot))
        (output / (stem + '.json')).write_text(json.dumps(result, indent=2))
        if result.get('error') or result.get('exit') != 0:
            raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=-1, collect_notebook=True, interaction=interaction)
