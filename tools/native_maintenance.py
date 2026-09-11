#!/usr/bin/env python3
"""Compact a shared section between two halves of a twelve-client editing run."""
import argparse
import base64
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
import xml.etree.ElementTree as ET

from native_runner import ROOT, windows
import linux_vm
from verify_smb_overlap import verify


def maintenance_locks(events):
    pending, files, peers = {}, {}, {}
    phase, attempts = None, []
    for event in events:
        assert not event.get('trace_error') and not event.get('encrypted')
        phase = event.get('control', {}).get('phase', phase)
        connection = event.get('connection')
        if event.get('opened'): peers[connection] = event['peer'][0]
        if event.get('closed'):
            files = {key: value for key, value in files.items() if key[0] != connection}
        if 'command' not in event: continue
        key = connection, event['message']
        if event['direction'] == 'request':
            pending[key] = event, phase
            continue
        if event['status'] == '0x103': continue
        pair = pending.pop(key, None)
        if pair is None: continue
        request, issued = pair
        command = request['command']
        if command == 5 and event['status'] == '0x0':
            files[connection, event['file_id']] = request['path'].replace('\\', '/').lower()
        elif command == 6:
            files.pop((connection, request['file_id']), None)
        elif command == 10 and peers[connection].startswith('192.168.77.'):
            path = files.get((connection, request['file_id']), '')
            if path != 'm6-collaboration/synthetic.one': continue
            for offset, length, flags in request['locks']:
                if (offset, length) == (0xffffeffc, 4096) and flags & 3 == 2:
                    attempts.append({'phase': issued, 'status': event['status'], 'connection': connection,
                                     'message': event['message'], 'flags': flags})
    assert any(a['phase'] == 'maintenance-held' and a['status'] in ('0xc0000054', '0xc0000055') for a in attempts), 'No section maintenance conflict observed while the reader held its guard'
    assert any(a['phase'] == 'maintenance-released' and a['status'] == '0x0' for a in attempts), 'No section maintenance guard acquired after reader release'
    return attempts


def run(output, server, fixture, operations, seed):
    output = output.resolve()
    assert not output.exists(), 'Choose a new output directory'
    output.parent.mkdir(parents=True, exist_ok=True)
    guardian = None
    with output.with_suffix('.log').open('x') as log:
        process = subprocess.Popen([sys.executable, ROOT / 'tools/native_collaboration.py', output,
            '--linux', server, '--stress-clients', '4', '--rust-writers', '4', '--rust-readers', '4',
            '--stress-operations', str(operations), '--seed', str(seed), '--edit', '--embedded-smb',
            '--maintenance', '--fixture', fixture], stdout=log, stderr=subprocess.STDOUT)

        def wait_for(predicate, timeout, message):
            deadline = time.monotonic() + timeout
            while not predicate():
                if process.poll() is not None: raise RuntimeError(f'Workload exited with {process.returncode}: {message}')
                if guardian is not None and guardian.poll() is not None and guardian.returncode != 0:
                    raise RuntimeError('The reader guardian failed')
                if time.monotonic() > deadline: raise TimeoutError(message)
                time.sleep(.2)

        def ssh(command):
            result = linux_vm.run_ssh(server, command, timeout=30)
            with (output / 'maintenance-server.jsonl').open('a') as stream:
                stream.write(json.dumps({'command': command, 'exit': result.returncode, 'stdout': result.stdout, 'stderr': result.stderr}) + '\n')
            result.check_returncode()
            return result.stdout

        def phase(name):
            ssh('printf \'{"phase":"' + name + '"}\' > /tmp/smb-control.json')
            wait_for(lambda: name in ssh(f'grep -F \'"control": {{"phase": "{name}"}}\' /tmp/smb-trace.jsonl || true'),
                     10, 'Proxy did not acknowledge the maintenance phase')

        def stat():
            inode, size = ssh("stat -c '%i %s' /srv/agent/m6-collaboration/synthetic.one").split()
            return {'inode': int(inode), 'size': int(size)}

        def ui(name, script):
            (output / f'{name}.ahk').write_text(script)
            result = windows.do_exec(script, target=target, timeout_ms=60000, shot_delay_ms=500)
            screenshot = result.pop('png_b64', None)
            if screenshot: (output / f'{name}.png').write_bytes(base64.b64decode(screenshot))
            (output / f'{name}.json').write_text(json.dumps(result, indent=2))
            if result.get('error') or result.get('exit') != 0: raise RuntimeError(str(result))

        try:
            wait_for(lambda: (output / 'maintenance-ready').exists(), 900, 'Native clients did not reach the initial checkpoint')
            (output / 'maintenance-start').touch()
            shared, rust = output / 'mount/m6-collaboration', output / 'rust'
            wait_for(lambda: all((shared / f'maintenance-paused-n{i}').exists() and (rust / f'paused-w{i}').exists() for i in range(4)),
                     300, 'Writers did not reach the maintenance barrier')
            (rust / 'pause').touch()
            wait_for(lambda: all((rust / f'paused-r{i}').exists() for i in range(4)), 60, 'Readers did not pause')
            target = json.loads((output / 'n0/machine.json').read_text())['name']
            page = ET.parse(sorted((output / 'n0').glob('snapshot-*.xml'))[-1]).getroot().attrib['ID']
            ui('maintenance-options', f'''if A_ScreenWidth != 800 || A_ScreenHeight != 600
    throw Error("The maintenance controller requires an 800 by 600 desktop.")
ComObject("OneNote.Application").NavigateTo("{page}", "", false)
WinWait("ahk_exe ONENOTE.EXE", , 10)
WinActivate("ahk_exe ONENOTE.EXE")
WinWaitActive("ahk_exe ONENOTE.EXE", , 10)
Send("!ft")
WinWait("OneNote Options", , 10)
WinActivate("OneNote Options")
WinWaitActive("OneNote Options", , 10)
Sleep(1000)
CoordMode("Mouse", "Screen")
Click(74, 134)
Sleep(1000)
''')
            hold = output / 'guardian'
            hold.mkdir()
            config = json.loads((output / 'linux.json').read_text())
            with (hold / 'run.log').open('w') as guardian_log:
                guardian = subprocess.Popen(['cargo', 'test', '-p', 'notebook', '--features', 'smb', 'live_reader_hold', '--', '--ignored', '--nocapture'],
                    cwd=ROOT, stdout=guardian_log, stderr=subprocess.STDOUT,
                    env={**os.environ, 'ONESTORE_SMB_LAB': f'127.0.0.1:{config["samba_port"]}',
                         'ONESTORE_SMB_PATH': 'm6-collaboration/synthetic.one', 'ONESTORE_SMB_HOLD': str(hold)})
                wait_for(lambda: (hold / 'ready').exists(), 60, 'Reader guard was not acquired')
                before = stat()
                phase('maintenance-held')
                ui('maintenance-denied', '''CoordMode("Mouse", "Screen")
WinActivate("OneNote Options")
WinWaitActive("OneNote Options", , 10)
Click(254, 440)
if !WinWait("Microsoft OneNote ahk_class #32770", , 30)
    throw Error("The maintenance conflict dialog did not appear.")
FileAppend(WinGetText("Microsoft OneNote ahk_class #32770"), "*")
''')
                held = stat()
                assert held == before, 'Section changed while the reader held its maintenance exclusion guard'
                (hold / 'release').touch()
                wait_for(lambda: guardian.poll() is not None, 30, 'Reader guardian did not release')
                assert guardian.returncode == 0
                assert json.loads((hold / 'released.json').read_text())['accepted'] > 0
            phase('maintenance-released')
            ui('maintenance-dismiss', '''WinActivate("Microsoft OneNote ahk_class #32770")
ControlClick("Button1", "Microsoft OneNote ahk_class #32770")
if !WinWaitClose("Microsoft OneNote ahk_class #32770", , 10)
    throw Error("The maintenance conflict dialog did not close.")
''')
            replacements = []
            for attempt in range(5):
                ui(f'maintenance-optimize-{attempt}', '''CoordMode("Mouse", "Screen")
WinActivate("OneNote Options")
WinWaitActive("OneNote Options", , 10)
Click(254, 440)
Sleep(3000)
if WinExist("Microsoft OneNote ahk_class #32770") {
    FileAppend(WinGetText("Microsoft OneNote ahk_class #32770"), "*")
    ControlClick("Button1", "Microsoft OneNote ahk_class #32770")
    if !WinWaitClose("Microsoft OneNote ahk_class #32770", , 10)
        throw Error("The maintenance conflict dialog did not close.")
}
''')
                replacements.append(stat())
                if replacements[-1]['inode'] != before['inode'] and replacements[-1]['size'] < before['size']: break
                time.sleep(2)
            (output / 'maintenance-stat.json').write_text(json.dumps({'before': before, 'held': held, 'attempts': replacements}, indent=2))
            assert replacements[-1]['inode'] != before['inode'] and replacements[-1]['size'] < before['size'], 'Native optimization did not replace and shrink the section'
            ui('maintenance-options-close', '''WinActivate("OneNote Options")
CoordMode("Mouse", "Screen")
Click(663, 533)
WinWaitClose("OneNote Options", , 10)
''')
            phase('maintenance-resumed')
            (rust / 'resume').touch()
            (shared / 'maintenance-resume').touch()
            wait_for(lambda: process.poll() is not None, 600, 'Post-maintenance workload did not complete')
            assert process.returncode == 0, 'Mixed workload failed after maintenance'
            events = [json.loads(line) for line in (output / 'smb-trace.jsonl').read_text().splitlines()]
            result = {'maintenance_locks': maintenance_locks(events), 'resumed_overlap': verify(events, phase='maintenance-resumed')}
            (output / 'maintenance-verification.json').write_text(json.dumps(result, indent=2))
        finally:
            if guardian is not None and guardian.poll() is None:
                (output / 'guardian/release').touch()
                try: guardian.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    guardian.terminate()
                    guardian.wait(timeout=30)
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=180)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--linux', required=True)
    parser.add_argument('--fixture', type=Path, required=True)
    parser.add_argument('--operations', type=int, default=40)
    parser.add_argument('--seed', type=int, default=908)
    args = parser.parse_args()
    if args.operations < 4 or args.operations % 2: parser.error('Use an even operation count of at least four.')
    def interrupted(_signal, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    run(args.output, args.linux, args.fixture.resolve(), args.operations, args.seed)
