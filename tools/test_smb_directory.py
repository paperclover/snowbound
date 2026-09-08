#!/usr/bin/env python3
"""Compare SMB enumeration with a disposable Linux VM's filesystem and interrupt pagination."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent / 'w7'))
from linux_vm import load_instance, ssh_argv


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('vm', help='An already running, caller-owned Linux lab VM')
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    config = load_instance(args.vm)
    root = 'directory-' + uuid.uuid4().hex[:12]
    script = f'''from pathlib import Path
import json
root = Path('/srv/agent') / {root!r}
root.mkdir()
(root / 'empty').mkdir()
(root / 'nested 🦀').mkdir()
(root / 'denied').mkdir()
(root / 'file.one').write_bytes(b'fixture')
for i in range(4000):
    name = f'Section {{i:04d}} 🦀 é ' + 'x' * 80 + '.one'
    with (root / name).open('wb') as file: file.truncate(i * 12345)
entries = [dict(name=p.name, directory=p.is_dir(), size=p.stat().st_size) for p in root.iterdir()]
(root / 'denied').chmod(0)
print(json.dumps(dict(path=root.name, entries=entries), ensure_ascii=False))
'''
    fixture = subprocess.run(ssh_argv(args.vm, 'python3 -'), input=script, text=True,
                             capture_output=True, check=True, timeout=60)
    (output / 'oracle.json').write_text(fixture.stdout)
    cases = [('complete', {})]
    cases += [(f'{direction}-{occurrence}', dict(cut=14, direction=direction, occurrence=occurrence))
              for direction in ('request', 'response') for occurrence in (1, 2, 10)]
    cases.append(('close', dict(cut=6, direction='response')))
    for name, control in cases:
        with socket.socket() as reservation:
            reservation.bind(('127.0.0.1', 0))
            port = reservation.getsockname()[1]
        control_path = output / f'{name}-control.json'
        control_path.write_text(json.dumps(control))
        trace = output / f'{name}-trace.jsonl'
        with trace.open('w') as log:
            proxy = subprocess.Popen([sys.executable, str(Path(__file__).with_name('smb-proxy.py')),
                                      str(control_path), '--port', str(port), '--server', '127.0.0.1',
                                      '--server-port', str(config['samba_port'])], stdout=log, stderr=log)
            try:
                deadline = time.monotonic() + 5
                while True:
                    records = [json.loads(line) for line in trace.read_text().splitlines()]
                    if any('listening' in row for row in records) and any('control' in row for row in records):
                        break
                    if proxy.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError('Directory test proxy did not start')
                    time.sleep(.05)
                env = dict(os.environ, ONESTORE_SMB_LAB=f'127.0.0.1:{port}',
                           ONESTORE_SMB_DIRECTORY=root, ONESTORE_SMB_DIRECTORY_ORACLE=str(output / 'oracle.json'))
                test = 'tests::live_directory_interruption' if control else 'tests::live_directory'
                with (output / f'{name}.log').open('w') as result:
                    subprocess.run(['cargo', 'test', '-p', 'onestore-smb', test, '--', '--ignored', '--exact'],
                                   env=env, stdout=result, stderr=result, check=True, timeout=120)
            finally:
                proxy.terminate()
                proxy.wait(timeout=5)
        records = [json.loads(line) for line in trace.read_text().splitlines()]
        if control:
            assert sum('cut' in row for row in records) == 1
        else:
            assert sum(row.get('command') == 14 and row.get('status') == '0x0' for row in records) > 10
            assert any(row.get('command') == 14 and row.get('status') == '0x80000006' for row in records)
        print(f'{name}: passed', flush=True)
    env = dict(os.environ, ONESTORE_SMB_LAB=f'127.0.0.1:{config["samba_port"]}',
               ONESTORE_SMB_DIRECTORY_ORACLE=str(output / 'oracle.json'))
    with (output / 'reconnected.log').open('w') as result:
        subprocess.run(['cargo', 'test', '-p', 'onestore-smb', 'tests::live_directory', '--', '--ignored', '--exact'],
                       env=env, stdout=result, stderr=result, check=True, timeout=120)
    print('reconnected: passed', flush=True)


if __name__ == '__main__':
    main()
