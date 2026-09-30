"""Delete and type over selections crossing a table's edge in OneNote 2010 and capture what
it stores. The oracle for the canvas editor's cross-container deletion.

Usage: native_cross_container.py NOTEBOOK_DIR OUTPUT_DIR
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

# Each page's keys once the caret stands in the line clicked: from "Before" into "Alpha one",
# from "Alpha two" out to "After", from "Before" over the whole first row into "Beta one",
# and from "Before" over the whole table into "After".
KEYS = {
    'Delete into a table': ((160, 236), '{Home}{Right 6}', 11, 'Send("{Delete}")'),
    'Delete out of a table': ((250, 260), '{Home}{Right 5}', 28, 'Send("{Delete}")\nSleep(500)\nSendText("X")'),
    'Type over a row': ((160, 236), '{Home}{Right 6}', 30, 'SendText("Y")'),
    'Delete a whole table': ((160, 236), '{Home}{Right 6}', 49, 'Send("{Delete}")'),
}


def interaction(name, output):
    pages = {}
    for path in sorted((output / 'read').glob('page-*.xml')):
        page = ET.parse(path).getroot()
        title = page.find('one:Title/one:OE/one:T', ns)
        if title is not None and title.text in KEYS:
            pages[title.text] = page.attrib['ID']
    for title, ((x, y), place, extend, action) in KEYS.items():
        script = '''ComObject("OneNote.Application").NavigateTo("%s", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
CoordMode("Mouse", "Screen")
Click(%d, %d)
Sleep(500)
Send("%s")
Sleep(300)
Send("+{Right %d}")
Sleep(500)
%s
Sleep(2000)
''' % (pages[title], x, y, place, extend, action)
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
