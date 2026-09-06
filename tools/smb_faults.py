#!/usr/bin/env python3
"""Run the embedded transport message-loss matrix on an owned Samba VM."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import time

from native_runner import ROOT
import linux_vm


def run(output, server):
    if linux_vm.instance_path(server).exists():
        raise ValueError('Choose a new Linux VM name.')
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    sources = ['tools/smb_faults.py', 'tools/smb-proxy.py', 'Cargo.lock',
               'crates/onestore-smb/src/lib.rs', 'crates/onestore-smb/src/tests.rs',
               'crates/onestore-smb/src/tests/faults.rs', 'crates/onestore/src/commit.rs',
               'crates/onestore/src/snapshot.rs']
    for name in sources:
        target = output / 'harness' / name
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(ROOT / name, target)
    (output / 'run.json').write_text(json.dumps({'server': server,
        'source_sha256': {name: hashlib.sha256((output / 'harness' / name).read_bytes()).hexdigest() for name in sources}}, indent=2))
    process = None
    try:
        linux_vm.create_instance(server)
        linux_vm.launch(server)
        linux_vm.wait_instance(server, 600)
        config = linux_vm.load_instance(server)
        (output / 'linux.json').write_text(json.dumps(config, indent=2))
        with (output / 'test.log').open('w') as log:
            process = subprocess.Popen(['cargo', 'test', '-p', 'onestore-smb', 'live_message_loss', '--', '--ignored', '--nocapture'],
                cwd=ROOT, stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                env={**os.environ, 'ONESTORE_SMB_LAB': f'127.0.0.1:{config["samba_port"]}',
                     'ONESTORE_SMB_EVIDENCE': str(output / 'cases')})
            if process.wait(timeout=900) != 0: raise RuntimeError('The message-loss matrix failed; inspect test.log.')
        hashes = linux_vm.run_ssh(server, 'sha256sum /srv/agent/fault-*.one', timeout=30)
        hashes.check_returncode()
        (output / 'server-sha256.txt').write_text(hashes.stdout)
        hashes = {Path(path).name: digest for digest, path in (line.split() for line in hashes.stdout.splitlines())}
        results = json.loads((output / 'cases/results.json').read_text())
        for result in results:
            captured = output / 'cases/recovered' / (result['case'] + '.one')
            assert hashlib.sha256(captured.read_bytes()).hexdigest() == hashes[result['path']], 'Recovered bytes differ from the independent server read'
        (output / 'verification.json').write_text(json.dumps({'cases': len(results), 'server_hashes_match': True}, indent=2))
    finally:
        if process is not None and process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            process.wait(timeout=30)
        if linux_vm.instance_path(server).exists():
            if linux_vm.running(server):
                try:
                    with (output / 'server.tar').open('wb') as archive:
                        subprocess.run(linux_vm.ssh_argv(server, 'tar cf - -C /srv/agent .'), stdout=archive, check=True, timeout=30)
                    result = linux_vm.run_ssh(server, 'sudo smbstatus --byterange --json', timeout=10)
                    (output / 'server-locks.json').write_text(result.stdout)
                finally:
                    try: linux_vm.shutdown(server, 60)
                    finally:
                        if linux_vm.running(server): linux_vm.qmp(server, 'quit')
                        deadline = time.monotonic() + 10
                        while linux_vm.running(server) and time.monotonic() < deadline: time.sleep(.1)
            linux_vm.delete_instance(server)
        (output / 'teardown.json').write_text(json.dumps({'linux_absent': not linux_vm.instance_path(server).exists()}, indent=2))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('output', type=Path)
    parser.add_argument('--linux', required=True)
    args = parser.parse_args()
    def interrupted(_signal, _frame): raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupted)
    run(args.output, args.linux)
