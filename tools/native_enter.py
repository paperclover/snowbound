"""Press Enter at the edges of and inside equations and links in OneNote 2010 on a
Rust-written page and capture what it stores. The oracle for the canvas editor's Enter.

Usage: native_enter.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows


def text(value):
    return 'SendText("%s")' % value.replace('"', '""')


def interaction(name, output):
    lines = [
        # Enter at an equation's start moves it to a paragraph of its own.
        text('Before '), 'Send("!=")', 'Sleep(600)', text('y^2+1'), 'Send(" ")', 'Sleep(700)',
        'Send("{Home}")', 'Send("{Right 7}")', 'Sleep(300)', 'Send("{Enter}")', 'Sleep(600)',
        # Enter inside the equation's own row breaks its line.
        'Send("{End}")', 'Send("{Left 2}")', 'Sleep(300)', 'Send("{Enter}")', 'Sleep(600)',
        # Enter inside a fraction's denominator makes it an equation array.
        'Send("{End}{Enter}")', 'Send("!=")', 'Sleep(600)', text('(a+b)/(c+d)'), 'Send(" ")',
        'Sleep(700)', 'Send("{Left 3}")', 'Sleep(300)', 'Send("{Enter}")', 'Sleep(600)',
        # Enter at a link's start moves the link down whole (inside one, it follows the link).
        'Send("^{End}{Enter}")', text('Select me please'), 'Send("+^{Left}+^{Left}")', 'Send("^k")',
        'Sleep(1500)', text('https://example.com/selected'), 'Send("{Enter}")', 'Sleep(800)',
        'Send("{End}")', 'Send("{Left 9}")', 'Sleep(300)', 'Send("{Enter}")', 'Sleep(600)',
    ]
    page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
    script = '''ComObject("OneNote.Application").NavigateTo("PAGE_ID", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
CoordMode("Mouse", "Screen")
Click(200, 140)
Sleep(500)
Send("{End}")
Sleep(300)
Send("{Enter}")
Sleep(500)
%s
Sleep(2000)
''' % '\n'.join(lines)
    script = script.replace('PAGE_ID', page)
    (output / 'enter.ahk').write_text(script)
    result = windows.do_exec(script, target=name, timeout_ms=240000, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / 'enter.png').write_bytes(base64.b64decode(screenshot))
    (output / 'enter.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=1, collect_notebook=True, interaction=interaction)
