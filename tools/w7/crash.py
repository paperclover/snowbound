"""Abruptly stop a disposable lab VM while preserving its disk for recovery."""
import argparse
import json
import os
import signal
import subprocess
import time

import linux_vm
import vm


def stop(kind, name):
    backend = {'windows': vm, 'linux': linux_vm}[kind]
    backend.load_instance(name)
    if not backend.running(name):
        raise ValueError('Choose a running disposable VM.')
    _, socket_path, pid_path, *_ = backend.runtime(name)
    pid = int(pid_path.read_text())
    expected = ('OneNote Windows 7 ' if kind == 'windows' else 'OneNote Linux Samba ') + name
    if backend.qmp(name, 'query-name').get('name') != expected:
        raise ValueError('The VM identity differs from its registered name.')
    command = subprocess.check_output(['ps', '-p', str(pid), '-o', 'command='], text=True)
    if str(socket_path) not in command or 'qemu-system-' not in command:
        raise ValueError('The registered process does not own this VM socket.')
    started = time.time_ns()
    os.kill(pid, signal.SIGKILL)
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            os.waitpid(pid, os.WNOHANG)
        except ChildProcessError:
            pass
        if not backend.running(name):
            break
        time.sleep(.1)
    if backend.running(name):
        raise TimeoutError('The VM process did not stop.')
    return {'kind': kind, 'name': name, 'pid': pid, 'signal': 'SIGKILL',
            'started_ns': started, 'stopped_ns': time.time_ns(),
            'machine_preserved': backend.instance_path(name).exists()}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('kind', choices=['windows', 'linux'])
    parser.add_argument('name')
    args = parser.parse_args()
    print(json.dumps(stop(args.kind, args.name), indent=2))
