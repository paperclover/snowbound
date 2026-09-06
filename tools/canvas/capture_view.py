#!/usr/bin/env python3
"""Capture a native page at 100% in a disposable clone held by native_runner.py --inspect."""
import argparse
import base64
import hashlib
import json
from pathlib import Path
import re
import sys
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / 'tools/w7'))
import mcp_win7 as windows


def capture(target, page, output, scrolls):
    machine = json.loads((page.parent.parent / 'machine.json').read_text())
    if machine['name'] != target:
        raise ValueError('The native capture belongs to a different disposable clone')
    page_id = ET.parse(page).getroot().attrib['ID']
    if not re.fullmatch(r'\{[0-9A-Fa-f-]+\}\{[0-9]+\}\{[0-9A-Fa-f]+\}', page_id):
        raise ValueError('Invalid native page identity')
    if not 0 <= scrolls <= 20:
        raise ValueError('Choose between zero and twenty scroll steps')
    output.mkdir(parents=True, exist_ok=False)
    setup = '''
dm := Buffer(220, 0)
NumPut("UShort", 220, dm, 68)
if !DllCall("EnumDisplaySettingsW", "Ptr", 0, "UInt", -1, "Ptr", dm)
    throw Error("Unable to read display mode")
if NumGet(dm, 172, "UInt") != 1600 || NumGet(dm, 176, "UInt") != 900 {
    NumPut("UInt", 0x180000, dm, 72)
    NumPut("UInt", 1600, dm, 172)
    NumPut("UInt", 900, dm, 176)
    result := DllCall("ChangeDisplaySettingsW", "Ptr", dm, "UInt", 0, "Int")
    if result != 0
        throw Error("Display mode rejected: " result)
}
Sleep(300)
app := ComObject("OneNote.Application")
'''
    setup += f'app.NavigateTo("{page_id}", "", false)\n'
    setup += '''
hwnd := WinWait("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE",, 10)
WinMaximize(hwnd)
WinActivate(hwnd)
WinWaitActive(hwnd,, 10)
Send("{Esc}")
Sleep(200)
Send("!w")
Sleep(200)
Send("q")
Sleep(100)
Send("^a")
SendText("100%")
Send("{Enter}{Esc}")
Sleep(200)
Send("!w")
dc := DllCall("GetDC", "Ptr", 0, "Ptr")
FileAppend("dpi=" DllCall("GetDeviceCaps", "Ptr", dc, "Int", 88) "," DllCall("GetDeviceCaps", "Ptr", dc, "Int", 90) "`n", "*")
DllCall("ReleaseDC", "Ptr", 0, "Ptr", dc)
FileAppend("screen=" SysGet(0) "x" SysGet(1) "`n", "*")
'''
    steps = [('zoom', setup), ('top', '''
Send("{Esc}{Esc}")
WinActivate("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE")
MouseMove(1000, 400)
Send("{WheelUp 100}")
MouseMove(800, 12)
''')]
    steps += [(f'scroll-{index + 1}', '''
MouseMove(1000, 400)
Send("{WheelDown 8}")
MouseMove(800, 12)
''') for index in range(scrolls)]
    records = []
    for name, script in steps:
        script = 'OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))\n' + script
        if name != 'zoom':
            script += '\nhwnd := WinExist("ahk_class Framework::CFrame ahk_exe ONENOTE.EXE")\nDllCall("RedrawWindow", "Ptr", hwnd, "Ptr", 0, "Ptr", 0, "UInt", 0x585)\n'
        (output / f'{name}.ahk').write_text(script)
        result = windows.do_exec(script, target=target, timeout_ms=30000, shot_delay_ms=700)
        record = {key: value for key, value in result.items() if key != 'png_b64'}
        (output / f'{name}.json').write_text(json.dumps(record, indent=2) + '\n')
        if result.get('error') or result.get('exit') != 0 or not result.get('png_b64'):
            raise RuntimeError(f'Native capture failed: {name}; inspect its JSON record')
        data = base64.b64decode(result['png_b64'], validate=True)
        (output / f'{name}.png').write_bytes(data)
        records.append({'frame': name, 'sha256': hashlib.sha256(data).hexdigest()})
    (output / 'manifest.json').write_text(json.dumps({
        'target': target, 'source_xml_name': page.name, 'source_page_id': page_id,
        'source_sha256': hashlib.sha256(page.read_bytes()).hexdigest(),
        'zoom_percent': 100, 'screen': [1600, 900], 'wheel_notches_per_step': 8,
        'frames': records,
    }, indent=2) + '\n')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('target')
    parser.add_argument('page_xml', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--scrolls', type=int, default=0)
    args = parser.parse_args()
    capture(args.target, args.page_xml, args.output, args.scrolls)
