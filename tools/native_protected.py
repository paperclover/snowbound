"""Cold-open a notebook whose sections Snowbound protected in a fresh OneNote 2010, unlock each
through OneNote's Protected Section dialog with its password, type into one through COM as an
add-in may, and read every page back with the notebook OneNote leaves.

Usage: native_protected.py NOTEBOOK_DIR PASSWORDS_JSON OUTPUT_DIR [EDITED_SECTION]
"""
import base64
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import native_runner as runner

windows = runner.windows
ROOT = Path(__file__).resolve().parent.parent
REMOTE = 'C:\\one-tests\\runs\\capture'


def unlock_script(password):
    return '''WinActivate("ahk_class Framework::CFrame")
WinWaitActive("ahk_class Framework::CFrame", , 30)
Sleep(2000)
Send("{Enter}")
if !WinWait("Protected Section", , 15)
    throw Error("No Protected Section dialog")
Sleep(500)
SendText("%s")
Send("{Enter}")
Sleep(4000)
FileAppend(WinGetTitle("A"), "*")
''' % password.replace('"', '""')


def interaction(passwords, edited):
    def run(name, output):
        result = windows.do_put(ROOT / 'tools/native/protected.ps1', 'C:\\one-tests\\protected.ps1', name)
        if result.get('error'):
            raise RuntimeError(result['error'])
        script = 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\protected.ps1 -Root %s -Section "%s"'
        for section, password in passwords.items():
            if password is None:
                continue
            runner.command(name, script % (REMOTE, section), output)
            result = windows.do_exec(unlock_script(password), target=name, timeout_ms=60000, shot_delay_ms=1000)
            stem = Path(section).stem
            (output / f'unlock-{stem}.json').write_text(json.dumps({k: v for k, v in result.items() if k != 'png_b64'}, indent=2))
            if result.get('png_b64'):
                (output / f'unlock-{stem}.png').write_bytes(base64.b64decode(result['png_b64']))
            if result.get('error') or result.get('exit') != 0:
                raise RuntimeError(f'Unlocking {section} failed; inspect unlock-{stem}.json.')
        if edited:
            runner.command(name, (script + ' -Edit') % (REMOTE, edited), output, 120000)
    return run


if __name__ == '__main__':
    if len(sys.argv) not in (4, 5):
        raise SystemExit(__doc__)
    notebook, passwords, output = Path(sys.argv[1]), json.loads(Path(sys.argv[2]).read_text()), Path(sys.argv[3])
    edited = sys.argv[4] if len(sys.argv) == 5 else None
    runner.capture(notebook, output, collect_notebook=True, interaction=interaction(passwords, edited))
    print('Captured native evidence in', output)
