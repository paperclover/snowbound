"""Draw ink over an outline's text and in empty space with the mouse in OneNote 2010 on a
Rust-written page, then capture the COM read: the oracle for handwriting held by paragraphs.

Usage: native_handwriting.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows


def interaction(name, output):
    page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
    script = '''ComObject("OneNote.Application").NavigateTo("PAGE_ID", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
SendMode("Event")
CoordMode("Mouse", "Screen")
Click(258,36)
Sleep(600)
Click(205,69)
Sleep(600)
; handwriting inside the outline, below its text line
MouseMove(140,160)
Click("Down")
MouseMove(160,175,25)
MouseMove(180,160,25)
MouseMove(200,175,25)
MouseMove(220,160,25)
Click("Up")
Sleep(300)
MouseClickDrag("Left",240,158,300,178,25)
Sleep(300)
; a drawing in empty page space
MouseMove(450,320)
Click("Down")
MouseMove(500,360,25)
MouseMove(450,400,25)
MouseMove(400,360,25)
MouseMove(450,320,25)
Click("Up")
Sleep(1000)
Click(21,74)
Sleep(2000)
'''.replace('PAGE_ID', page)
    (output / 'handwriting.ahk').write_text(script)
    result = windows.do_exec(script, target=name, timeout_ms=120000, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / 'handwriting.png').write_bytes(base64.b64decode(screenshot))
    (output / 'handwriting.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=1, collect_notebook=True, interaction=interaction)
