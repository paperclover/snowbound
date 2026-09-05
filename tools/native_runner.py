#!/usr/bin/env python3
"""Capture a notebook with real OneNote in a disposable Windows clone."""
import argparse
import base64
from contextlib import contextmanager
import hashlib
import json
from pathlib import Path
import signal
import re
import xml.etree.ElementTree as ET
import sys
import time
import uuid
import zipfile

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / 'tools/w7'))
import mcp_win7 as windows
import vm


def command(target, text, output, timeout_ms=120000):
    result = windows.do_cmd(text, timeout_ms=timeout_ms, target=target)
    with (output / 'commands.jsonl').open('a') as log:
        log.write(json.dumps({'command': text, **result}) + '\n')
    if result.get('exit') != 0 or result.get('error'):
        raise RuntimeError(windows.text_result(result))


def install_agent(name, output):
    source = output / 'agent.py'
    source.write_bytes((ROOT / 'tools/w7/payload/agent.py').read_bytes())
    expected = hashlib.sha256(source.read_bytes()).hexdigest()
    restart = output / 'restart-agent.py'
    restart.write_bytes((ROOT / 'tools/w7/restart_agent.py').read_bytes())
    for local, remote in [(source, r'C:\win7-agent\agent.py'),
                          (restart, r'C:\one-tests\restart-agent.py')]:
        result = windows.do_put(local, remote, name)
        if result.get('error'):
            raise RuntimeError(result['error'])
    result = windows.do_spawn(r'C:\win7-agent\vendor\python\python.exe C:\one-tests\restart-agent.py ONE-' + name.upper(), name)
    if result.get('error'):
        raise RuntimeError(result['error'])
    deadline = time.monotonic() + 60
    last_result = None
    while time.monotonic() < deadline:
        try:
            health = windows.do_health(name)
            last_result = health
            if health.get('agent_sha256') == expected:
                return health
        except windows.Win7Error as error:
            last_result = str(error)
        time.sleep(.2)
    (output / 'agent-startup-failure.json').write_text(json.dumps({'expected_sha256': expected, 'last_result': last_result}, indent=2))
    raise RuntimeError('The updated clone agent did not become ready.')


def collect_artifacts(name, output, content, partial=False):
    command(name, 'powershell -NoProfile -Command "Compress-Archive -Force -Path C:\\one-tests\\runs\\capture\\%s -DestinationPath C:\\one-tests\\captured.zip"' % content, output)
    archive_path = output / 'capture.zip'
    result = windows.do_get('C:\\one-tests\\captured.zip', archive_path, name)
    if result.get('error') or not archive_path.is_file():
        raise RuntimeError('The native capture was not returned; inspect commands.jsonl.')
    with zipfile.ZipFile(archive_path) as archive:
        for entry in archive.infolist():
            relative = Path(entry.filename.replace('\\', '/'))
            if relative.is_absolute() or '..' in relative.parts:
                raise ValueError('The native archive contains a path outside the capture.')
            path = (output / 'failure-artifacts' if partial else output) / relative
            if entry.is_dir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(archive.read(entry))
    archive_path.unlink()


@contextmanager
def clone(output):
    name = 'm6-' + uuid.uuid4().hex[:8]
    if vm.instance_path(name).exists():
        raise RuntimeError('The generated clone name is already in use; rerun the capture.')
    try:
        vm.create_instance(name)
        (output / 'machine.json').write_text(json.dumps({'name': name, 'hostname': 'ONE-' + name.upper()}) + '\n')
        vm.start_instance(name)
        vm.wait_instance(name, 300)
        (output / 'health.json').write_text(json.dumps(install_agent(name, output), indent=2) + '\n')
        yield name
    except BaseException:
        if (output / 'health.json').exists():
            try:
                collect_artifacts(name, output, '*', partial=True)
            except Exception as failure:
                (output / 'artifact-failure.txt').write_text(str(failure))
        try:
            shot = windows.do_shot(name)
            if shot.get('png_b64'):
                (output / 'failure.png').write_bytes(base64.b64decode(shot['png_b64']))
        except Exception:
            pass
        raise
    finally:
        if vm.instance_path(name).exists():
            if vm.running(name):
                try:
                    vm.shutdown(name, 60)
                except (Exception, SystemExit):
                    vm.qmp(name, 'quit')
                    deadline = time.monotonic() + 10
                    while vm.running(name) and time.monotonic() < deadline:
                        time.sleep(0.1)
            vm.delete_instance(name)
        (output / 'teardown.json').write_text(json.dumps({'absent': not vm.instance_path(name).exists()}) + '\n')


def capture(notebook, output, expected_pages=-1, author=None, screenshots=False, pdf=False, author_timeout=600, inspect=False):
    notebook = notebook.resolve(strict=True)
    if not notebook.is_dir():
        raise ValueError('Choose a notebook directory.')
    if output.resolve().is_relative_to(notebook):
        raise ValueError('Choose an output directory outside the source notebook.')
    output.mkdir(parents=True, exist_ok=False)
    scripts = output / 'scripts'
    scripts.mkdir()
    for name in ('cold.ps1', 'read.ps1'):
        (scripts / name).write_bytes((ROOT / 'tools/native' / name).read_bytes())
    if author is not None:
        (scripts / 'author.ps1').write_bytes(author.read_bytes())
    (output / 'run.json').write_text(json.dumps({
        'notebook': str(notebook), 'expected_pages': expected_pages, 'author': str(author) if author else None,
        'author_timeout_seconds': author_timeout,
        'inspect': inspect,
        'base': json.loads(vm.BASE_MANIFEST.read_text()),
        'scripts': {name: hashlib.sha256((scripts / name).read_bytes()).hexdigest()
                    for name in sorted(p.name for p in scripts.iterdir())},
    }, indent=2) + '\n')
    source = []
    transfer = output / 'transfer.zip'
    with zipfile.ZipFile(transfer, 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(notebook.rglob('*')):
            if not path.is_file():
                continue
            relative = path.relative_to(notebook).as_posix()
            data = path.read_bytes()
            source.append({'path': relative, 'bytes': len(data), 'sha256': hashlib.sha256(data).hexdigest(),
                           'mtime_ns': path.stat().st_mtime_ns})
            archive.writestr(zipfile.ZipInfo.from_file(path, arcname=relative), data)
    (output / 'source.json').write_text(json.dumps(source, indent=2) + '\n')
    try:
        with clone(output) as name:
            for local, remote in [('cold.ps1', 'cold-current.ps1'), ('read.ps1', 'read-current.ps1')]:
                result = windows.do_put(scripts / local, 'C:\\one-tests\\' + remote, name)
                if result.get('error'):
                    raise RuntimeError(result['error'])
            result = windows.do_put(transfer, 'C:\\one-tests\\transfer.zip', name)
            if result.get('error'):
                raise RuntimeError(result['error'])
            command(name, 'powershell -NoProfile -Command "Expand-Archive -LiteralPath C:\\one-tests\\transfer.zip -DestinationPath C:\\one-tests\\runs\\capture\\notebook"', output)
            if author is not None:
                result = windows.do_put(scripts / 'author.ps1', 'C:\\one-tests\\author.ps1', name)
                if result.get('error'):
                    raise RuntimeError(result['error'])
                command(name, 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\author.ps1 -Root C:\\one-tests\\runs\\capture -CloneHost ONE-%s' % name.upper(), output, author_timeout * 1000)
            command(name, 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\read-current.ps1 -Root C:\\one-tests\\runs\\capture -CloneHost ONE-%s -ExpectedPages %d%s' % (name.upper(), expected_pages, (' -UseCurrentCache' if author else '') + (' -Pdf' if pdf else '') + (' -KeepOpen' if screenshots or inspect else '')), output, 600000)
            collect_artifacts(name, output, '*' if author else 'read')
            if screenshots:
                for page in sorted((output / 'read').glob('page-*.xml')):
                    page_id = ET.parse(page).getroot().attrib['ID']
                    if not re.fullmatch(r'\{[0-9A-Fa-f-]+\}\{[0-9]+\}\{[0-9A-Fa-f]+\}', page_id):
                        raise ValueError('The native page identity cannot be used for navigation.')
                    result = windows.do_exec(
                        'OnError((exception, mode) => (FileAppend(exception.Message, "**"), ExitApp(1)))\n'
                        'DetectHiddenWindows false\napp := ComObject("OneNote.Application")\n'
                        f'app.NavigateTo("{page_id}", "", false)\n'
                        'hwnd := WinWait("ahk_exe ONENOTE.EXE",, 10)\n'
                        'WinMaximize(hwnd)\nWinActivate(hwnd)\nWinWaitActive(hwnd,, 10)\n',
                        target=name, shot_delay_ms=1500)
                    page.with_suffix('.navigation.json').write_text(json.dumps({k: v for k, v in result.items() if k != 'png_b64'}, indent=2))
                    if result.get('png_b64'):
                        page.with_suffix('.png').write_bytes(base64.b64decode(result['png_b64']))
                    if result.get('error') or result.get('exit') != 0 or not result.get('png_b64'):
                        raise RuntimeError('The native page screenshot failed; inspect its navigation record.')
            if inspect:
                print(f'Inspection ready: {name}; create {output / "finish"} to capture and close it.', flush=True)
                deadline = time.monotonic() + 1800
                while not (output / 'finish').exists() and time.monotonic() < deadline:
                    time.sleep(.2)
                command(name, 'powershell -NoProfile -Command "Rename-Item C:\\one-tests\\runs\\capture\\read before-read"', output)
                (output / 'read').rename(output / 'before-read')
                command(name, 'powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File C:\\one-tests\\read-current.ps1 -Root C:\\one-tests\\runs\\capture -CloneHost ONE-%s -UseCurrentCache -ExpectedPages -1%s' % (name.upper(), ' -Pdf' if pdf else ''), output, 600000)
                collect_artifacts(name, output, '*')
        for item in source:
            path = notebook / item['path']
            if path.stat().st_mtime_ns != item['mtime_ns'] or hashlib.sha256(path.read_bytes()).hexdigest() != item['sha256']:
                raise RuntimeError('The source changed during capture; repeat from an isolated copy.')
    finally:
        transfer.unlink(missing_ok=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('notebook', type=Path)
    parser.add_argument('output', type=Path)
    parser.add_argument('--expected-pages', type=int, default=-1)
    parser.add_argument('--pdf', action='store_true')
    parser.add_argument('--screenshots', action='store_true')
    parser.add_argument('--inspect', action='store_true', help='Keep the clone open for inspection until an output/finish file appears, up to 30 minutes.')
    parser.add_argument('--author', type=Path, help='Run a fixture-authoring PowerShell script in the clone before capture.')
    parser.add_argument('--author-timeout', type=int, default=600, help='Authoring deadline in seconds.')
    args = parser.parse_args()
    def interrupted(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    if args.author_timeout <= 0:
        parser.error('The authoring deadline must be positive.')
    capture(args.notebook, args.output, args.expected_pages, args.author, args.screenshots, args.pdf, args.author_timeout, args.inspect)
    print('Captured native evidence in', args.output)
