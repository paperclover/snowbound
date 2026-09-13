"""Author equations through OneNote 2010's equation editor on a Rust-written page and capture
the COM read, which exports each equation as MathML: the oracle for `onestore::page::Math`.

Usage: native_math.py NOTEBOOK_DIR OUTPUT_DIR
"""
import base64
import json
import os
import sys
from pathlib import Path
import xml.etree.ElementTree as ET

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows

# Linear-format input, one equation per paragraph; a space builds each expression up.
EQUATIONS = [
    'a_1+b_2',
    '\\sqrt(x+1)',
    'x_i^2',
    '(a+b)',
    '\\int_0^1 x dx',
    '\\sum_(i=1)^n i',
]
if os.environ.get('ONESTORE_MATH_EQUATIONS'):
    EQUATIONS = os.environ['ONESTORE_MATH_EQUATIONS'].split('|')


def interaction(name, output):
    lines = []
    for equation in EQUATIONS:
        lines.append('Send("!=")')
        lines.append('Sleep(600)')
        lines.append('SendText("%s")' % equation.replace('"', '""'))
        lines.append('Send(" ")')
        lines.append('Sleep(600)')
        lines.append('Send("{End}")')
        lines.append('Sleep(300)')
        lines.append('Send("{Enter}")')
        lines.append('Sleep(600)')
    page = ET.parse(sorted((output / 'read').glob('page-*.xml'))[0]).getroot().attrib['ID']
    script = '''ComObject("OneNote.Application").NavigateTo("PAGE_ID", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 30)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 30)
WinMaximize("ahk_exe ONENOTE.EXE")
Sleep(3000)
CoordMode("Mouse", "Screen")
Click(300, 140)
Sleep(500)
Send("{End}")
Sleep(300)
Send("{Enter}")
Sleep(500)
%s
Sleep(2000)
''' % '\n'.join(lines)
    script = script.replace('PAGE_ID', page)
    (output / 'equations.ahk').write_text(script)
    result = windows.do_exec(script, target=name, timeout_ms=180000, shot_delay_ms=1500)
    screenshot = result.pop('png_b64', None)
    if screenshot:
        (output / 'equations.png').write_bytes(base64.b64decode(screenshot))
    (output / 'equations.json').write_text(json.dumps(result, indent=2))
    if result.get('error') or result.get('exit') != 0:
        raise RuntimeError(str(result))


if __name__ == '__main__':
    notebook, output = Path(sys.argv[1]), Path(sys.argv[2])
    runner.capture(notebook, output, expected_pages=1, collect_notebook=True, interaction=interaction)
